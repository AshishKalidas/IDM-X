//! "How much would combining my networks actually help?" — measured, not guessed.

use crate::engine::{Engine, Inner};
use crate::error::{Error, Result};
use crate::http::{build_client, ClientSpec};
use crate::net::local_addr;
use crate::types::*;
use futures_util::StreamExt;
use reqwest::Client;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const DEFAULT_URL: &str = "https://speed.cloudflare.com/__down?bytes=25000000";
const STREAMS: usize = 4;
const SOLO: Duration = Duration::from_millis(4000);
const COMBINED: Duration = Duration::from_millis(5000);
const WARMUP: Duration = Duration::from_millis(1200);

impl Engine {
    /// Start a speed test in the background; progress arrives as `Event::SpeedTest`.
    pub fn speed_test(&self, url: Option<String>) -> Result<()> {
        if self.inner.speedtest_busy.swap(true, Relaxed) {
            return Err(Error::msg("a speed test is already running"));
        }
        tokio::spawn(self.inner.clone().speedtest_task(url.filter(|u| !u.trim().is_empty()).unwrap_or_else(|| DEFAULT_URL.into())));
        Ok(())
    }
}

impl Inner {
    async fn speedtest_task(self: Arc<Self>, url: String) {
        let result = self.speedtest(&url).await;
        if let Err(e) = result {
            self.emit(Event::SpeedTest(SpeedTestReport {
                phase: "failed".into(),
                done: true,
                warnings: vec![e.to_string()],
                ..Default::default()
            }));
        }
        self.speedtest_busy.store(false, Relaxed);
    }

    fn client_for(&self, iface: &NetInterface) -> Result<Client> {
        build_client(&ClientSpec {
            local: local_addr(iface, false),
            device: self.bind_device.then(|| iface.id.clone()),
            proxy: self.settings.read().proxy.clone(),
            pin: None,
        })
    }

    async fn speedtest(self: &Arc<Self>, url: &str) -> Result<()> {
        let settings = self.settings.read().clone();
        let nets: Vec<NetInterface> = self
            .ifaces
            .read()
            .iter()
            .filter(|i| i.usable)
            .filter(|i| settings.interfaces.get(&i.id).and_then(|r| r.enabled).unwrap_or(i.kind.default_enabled()))
            .cloned()
            .collect();
        if nets.is_empty() {
            return Err(Error::NoNetwork);
        }
        let mut report = SpeedTestReport {
            phase: "Measuring each network".into(),
            nets: nets
                .iter()
                .map(|n| SpeedTestNet { id: n.id.clone(), label: n.label.clone(), kind: n.kind, mbps: 0.0, latency_ms: None, error: None })
                .collect(),
            ..Default::default()
        };
        self.emit(Event::SpeedTest(report.clone()));

        for (i, n) in nets.iter().enumerate() {
            report.phase = format!("Measuring {}", n.label);
            self.emit(Event::SpeedTest(report.clone()));
            match self.client_for(n) {
                Ok(c) => match measure(c, url.to_string(), STREAMS, SOLO).await {
                    Ok((mbps, lat)) => {
                        report.nets[i].mbps = mbps;
                        report.nets[i].latency_ms = lat;
                    }
                    Err(e) => report.nets[i].error = Some(e.to_string()),
                },
                Err(e) => report.nets[i].error = Some(e.to_string()),
            }
            self.emit(Event::SpeedTest(report.clone()));
        }

        report.sum_mbps = report.nets.iter().map(|n| n.mbps).sum();
        let ok: Vec<&NetInterface> = nets.iter().enumerate().filter(|(i, _)| report.nets[*i].error.is_none() && report.nets[*i].mbps > 0.0).map(|(_, n)| n).collect();

        if ok.len() >= 2 {
            report.phase = "Measuring all together".into();
            self.emit(Event::SpeedTest(report.clone()));
            let total = Arc::new(AtomicU64::new(0));
            let mut tasks = Vec::new();
            for n in &ok {
                if let Ok(c) = self.client_for(n) {
                    let (u, t) = (url.to_string(), total.clone());
                    tasks.push(tokio::spawn(async move { pump(c, u, STREAMS, COMBINED, t).await }));
                }
            }
            tokio::time::sleep(WARMUP).await;
            let at_warm = total.load(Relaxed);
            tokio::time::sleep(COMBINED - WARMUP).await;
            let bytes = total.load(Relaxed).saturating_sub(at_warm);
            for t in tasks {
                t.abort();
            }
            report.combined_mbps = Some(bytes as f64 * 8.0 / (COMBINED - WARMUP).as_secs_f64() / 1e6);
            let best = report.nets.iter().map(|n| n.mbps).fold(0.0, f64::max);
            if best > 0.0 {
                report.gain = report.combined_mbps.map(|c| c / best);
            }
            // Explain a disappointing result instead of leaving the user to guess.
            for (a, b) in shared_gateways(&ok) {
                report.warnings.push(format!("{a} and {b} share a gateway, so they draw on the same internet connection."));
            }
            if report.gain.is_some_and(|g| g < 1.25) && report.warnings.is_empty() {
                report.warnings.push("Combining barely helped. These networks may share an upstream link, or the server is the limit.".into());
            }
        } else if ok.len() == 1 {
            report.warnings.push("Only one network responded — there is nothing to combine yet.".into());
        }
        report.phase = "Done".into();
        report.done = true;
        self.emit(Event::SpeedTest(report));
        Ok(())
    }
}

fn shared_gateways(nets: &[&NetInterface]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (i, a) in nets.iter().enumerate() {
        for b in &nets[i + 1..] {
            if a.gateway.is_some() && a.gateway == b.gateway {
                out.push((a.label.clone(), b.label.clone()));
            }
        }
    }
    out
}

/// Pull `url` on `streams` connections until `dur` has passed, adding to `total`.
async fn pump(client: Client, url: String, streams: usize, dur: Duration, total: Arc<AtomicU64>) {
    let deadline = Instant::now() + dur;
    let mut tasks = Vec::new();
    for _ in 0..streams {
        let (c, u, t) = (client.clone(), url.clone(), total.clone());
        tasks.push(tokio::spawn(async move {
            while Instant::now() < deadline {
                let Ok(resp) = c.get(&u).send().await else { return };
                if !resp.status().is_success() {
                    return;
                }
                let mut s = resp.bytes_stream();
                while let Some(Ok(chunk)) = s.next().await {
                    t.fetch_add(chunk.len() as u64, Relaxed);
                    if Instant::now() >= deadline {
                        return;
                    }
                }
            }
        }));
    }
    for t in tasks {
        let _ = tokio::time::timeout(dur + Duration::from_secs(3), t).await;
    }
}

/// Megabits per second over the post-warm-up part of the window, plus time to first byte.
async fn measure(client: Client, url: String, streams: usize, dur: Duration) -> Result<(f64, Option<f64>)> {
    // Latency first, on its own connection.
    let t0 = Instant::now();
    // A plain GET: some speed-test hosts refuse requests that carry a Range header.
    let probe = client.get(&url).send().await?;
    let lat = t0.elapsed().as_secs_f64() * 1000.0;
    if !probe.status().is_success() {
        return Err(Error::Http(probe.status().as_u16()));
    }
    drop(probe);

    let total = Arc::new(AtomicU64::new(0));
    let t = total.clone();
    let task = tokio::spawn(async move { pump(client, url, streams, dur, t).await });
    tokio::time::sleep(WARMUP).await;
    let at_warm = total.load(Relaxed);
    tokio::time::sleep(dur - WARMUP).await;
    let bytes = total.load(Relaxed).saturating_sub(at_warm);
    task.abort();
    if bytes == 0 {
        return Err(Error::msg("no data received"));
    }
    Ok((bytes as f64 * 8.0 / (dur - WARMUP).as_secs_f64() / 1e6, Some(lat)))
}
