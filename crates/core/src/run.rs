//! One download in flight: a pool of streams, grouped by network, pulling blocks from a shared
//! table and writing them at their final offsets in the staging file.

use crate::blocks::{plan_block_size, BlockTable, Claim, HedgePolicy, Kind, SavedBlocks, NO_OWNER};
use crate::error::{Error, Result};
use crate::http::{build_client, parse_content_range, ranged, ClientSpec};
use crate::limiter::{acquire_all, Limiter};
use crate::net::local_addr;
use crate::types::{BlockMap, InterfaceRule, NetInterface, NetState, NetStat, StreamStat};
use crate::util::write_all_at;
use futures_util::StreamExt;
use parking_lot::{Mutex, RwLock};
use rand::Rng;
use reqwest::header::{HeaderMap, IF_RANGE};
use reqwest::{Client, StatusCode};
use std::fs::File;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{watch, Notify};
use tokio_util::sync::CancellationToken;

const FLUSH_AT: usize = 256 << 10;
const CONNECT_AND_TTFB: Duration = Duration::from_secs(30);
const MIN_STREAMS: u32 = 2;
/// Matched by `could_not_connect`, so keep them together.
const SLOW_SERVER: &str = "The server took too long to answer. IDMX will retry.";
const STRIKES_FATAL: u32 = 8;
const QUARANTINE_AFTER: u32 = 5;

pub type Refresh = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<String>> + Send>> + Send + Sync>;

// ───────────────────────────── meters ─────────────────────────────

/// Bytes counter with a smoothed bytes/second reading.
#[derive(Default)]
pub struct Meter {
    total: AtomicU64,
    speed: AtomicU64,
    last: Mutex<Option<(u64, Instant)>>,
}

impl Meter {
    pub fn add(&self, n: u64) {
        self.total.fetch_add(n, Relaxed);
    }
    pub fn total(&self) -> u64 {
        self.total.load(Relaxed)
    }
    pub fn speed(&self) -> f64 {
        f64::from_bits(self.speed.load(Relaxed))
    }
    pub fn sample(&self, now: Instant) {
        let total = self.total();
        let mut last = self.last.lock();
        if let Some((t0, at)) = *last {
            let dt = now.duration_since(at).as_secs_f64();
            if dt >= 0.1 {
                let inst = (total.saturating_sub(t0)) as f64 / dt;
                let prev = self.speed();
                // Smooth, but let a stop register quickly.
                let a = if inst < prev * 0.5 { 0.6 } else { 0.3 };
                self.speed.store((prev * (1.0 - a) + inst * a).to_bits(), Relaxed);
                *last = Some((total, now));
            }
        } else {
            *last = Some((total, now));
        }
    }
}

// ───────────────────────────── per-network runtime ─────────────────────────────

#[derive(Default)]
struct Health {
    lat_ewma: Option<f64>,
    ok: f64,
    bad: f64,
    consecutive_fail: u32,
    quarantines: u32,
}

/// Static inputs for one network taking part in a run.
pub struct NetPlan {
    pub iface: NetInterface,
    pub rule: InterfaceRule,
    pub streams: u32,
    pub limiter: Arc<Limiter>,
    /// Bytes used this period; shared across jobs.
    pub usage: Arc<AtomicU64>,
    /// Absolute cap on `usage`.
    pub cap: Option<u64>,
    /// Force egress through the device (macOS/Linux). Off in tests that use loopback.
    pub bind_device: bool,
    pub proxy: Option<String>,
    /// Whether the user has it switched on for this download. A disabled network is still part
    /// of the run so it can be switched on while the download is going.
    pub enabled: bool,
}

pub struct NetRt {
    idx: usize,
    iface: RwLock<NetInterface>,
    rule: InterfaceRule,
    client: RwLock<Client>,
    bind_device: bool,
    proxy: Option<String>,
    desired: AtomicU32,
    spawned: AtomicU32,
    max: u32,
    active: AtomicU32,
    meter: Meter,
    limiter: Arc<Limiter>,
    usage: Arc<AtomicU64>,
    cap: Option<u64>,
    state: AtomicU8,
    until: Mutex<Option<Instant>>,
    health: Mutex<Health>,
    refused: AtomicU32,
    errors: AtomicU32,
    epoch: watch::Sender<u64>,
    nudge: Notify,
    probation: AtomicBool,
    base_streams: u32,
    /// The user switched it off for this download.
    user_off: AtomicBool,
    /// What went wrong most recently; cleared when something gets through.
    last_error: Mutex<Option<String>>,
    /// Whether the last failure was the server turning us away (as opposed to not answering).
    refusal: AtomicBool,
    /// 4 or 6: the IP family this network's connections use. 0 = none in common with the server.
    family: AtomicU8,
    /// What the server resolved to, if known.
    target: Option<(String, u16, Vec<IpAddr>)>,
}

fn st_to_u8(s: NetState) -> u8 {
    match s {
        NetState::Idle => 0,
        NetState::Active => 1,
        NetState::Unreachable => 2,
        NetState::Failed => 3,
        NetState::Capped => 4,
        NetState::Offline => 5,
        NetState::Disabled => 6,
    }
}
fn u8_to_st(v: u8) -> NetState {
    match v {
        0 => NetState::Idle,
        1 => NetState::Active,
        2 => NetState::Unreachable,
        3 => NetState::Failed,
        4 => NetState::Capped,
        5 => NetState::Offline,
        _ => NetState::Disabled,
    }
}

/// Which IP family a network should use to reach a server that has these addresses: IPv4 when
/// both sides have it (it is the safer default), IPv6 when that's all they share. `None` when
/// there is nothing in common — a network with no IPv6 can't reach an IPv6-only server.
pub fn choose_family(iface: &NetInterface, server: &[IpAddr]) -> Option<u8> {
    let (s4, s6) = (server.iter().any(IpAddr::is_ipv4), server.iter().any(IpAddr::is_ipv6));
    let (i4, i6) = (!iface.ipv4.is_empty(), !iface.ipv6.is_empty());
    if server.is_empty() {
        // Nothing resolved (proxy, or a literal we couldn't classify): go with what we have.
        return Some(if i4 || !i6 { 4 } else { 6 });
    }
    if i4 && s4 {
        Some(4)
    } else if i6 && s6 {
        Some(6)
    } else {
        None
    }
}

fn other_family(f: u8) -> u8 {
    if f == 4 { 6 } else { 4 }
}

fn is_refusal(e: &Error) -> bool {
    matches!(e, Error::Http(_))
}

impl NetRt {
    /// Set up one network's runtime state from its plan.
    fn build(idx: usize, p: NetPlan, ranges: bool, max: u32, remaining_blocks: u32, n_nets: u32, target: Option<(String, u16, Vec<IpAddr>)>) -> NetRt {
        let base = p.streams.clamp(1, max);
        let share = if ranges { (remaining_blocks / n_nets.max(1)).max(1) } else { 1 };
        let desired = base.min(share.max(1));
        let (epoch, _) = watch::channel(0u64);
        let max_cap = p.rule_streams_cap(max);
        let family = match &target {
            Some((_, _, ips)) => choose_family(&p.iface, ips),
            None => choose_family(&p.iface, &[]),
        };
        let enabled = p.enabled;
        let rt = NetRt {
            idx,
            iface: RwLock::new(p.iface.clone()),
            rule: p.rule,
            client: RwLock::new(Client::new()),
            bind_device: p.bind_device,
            proxy: p.proxy,
            desired: AtomicU32::new(if ranges { desired } else { 1 }),
            spawned: AtomicU32::new(0),
            max: max_cap,
            active: AtomicU32::new(0),
            meter: Meter::default(),
            limiter: p.limiter,
            usage: p.usage,
            cap: p.cap,
            state: AtomicU8::new(st_to_u8(NetState::Active)),
            until: Mutex::new(None),
            health: Mutex::new(Health::default()),
            refused: AtomicU32::new(0),
            errors: AtomicU32::new(0),
            epoch,
            nudge: Notify::new(),
            probation: AtomicBool::new(false),
            base_streams: base,
            user_off: AtomicBool::new(!enabled),
            last_error: Mutex::new(match family {
                None => Some("This network and the server have no IP version in common (the server is IPv6-only and this network has no IPv6, or the reverse).".to_string()),
                Some(_) => None,
            }),
            refusal: AtomicBool::new(false),
            family: AtomicU8::new(family.unwrap_or(0)),
            target,
        };
        if !enabled {
            rt.set_state(NetState::Disabled);
        } else if family.is_none() {
            rt.set_state(NetState::Unreachable);
        }
        let _ = rt.rebuild_client();
        rt
    }

    fn state(&self) -> NetState {
        u8_to_st(self.state.load(Relaxed))
    }
    fn set_state(&self, s: NetState) {
        self.state.store(st_to_u8(s), Relaxed);
    }
    fn client(&self) -> Client {
        self.client.read().clone()
    }
    fn capped(&self) -> bool {
        self.cap.is_some_and(|c| self.usage.load(Relaxed) >= c)
    }
    fn runnable(&self) -> bool {
        self.state() == NetState::Active && !self.capped()
    }
    fn bump_epoch(&self) {
        self.epoch.send_modify(|e| *e += 1);
    }

    fn rebuild_client(&self) -> Result<()> {
        let iface = self.iface.read().clone();
        let fam = self.family.load(Relaxed);
        let want_v6 = fam == 6;
        let pin = self.target.as_ref().map(|(host, port, ips)| {
            let addrs: Vec<SocketAddr> = ips.iter().filter(|a| a.is_ipv6() == want_v6).map(|a| SocketAddr::new(*a, *port)).collect();
            (host.clone(), addrs)
        });
        let spec = ClientSpec {
            local: local_addr(&iface, want_v6).filter(|a| a.is_ipv6() == want_v6 || fam == 0),
            device: self.bind_device.then(|| iface.id.clone()),
            proxy: self.proxy.clone(),
            pin,
        };
        *self.client.write() = build_client(&spec)?;
        Ok(())
    }

    /// A connection couldn't be made over this family; try the other one if this network and the
    /// server both have it. Returns whether it switched.
    fn switch_family(&self) -> bool {
        let cur = self.family.load(Relaxed);
        if cur == 0 {
            return false;
        }
        let next = other_family(cur);
        let iface = self.iface.read().clone();
        let server = self.target.as_ref().map(|t| t.2.clone()).unwrap_or_default();
        let ok = match next {
            4 => !iface.ipv4.is_empty() && server.iter().any(IpAddr::is_ipv4),
            _ => !iface.ipv6.is_empty() && server.iter().any(IpAddr::is_ipv6),
        };
        if !ok {
            return false;
        }
        self.family.store(next, Relaxed);
        self.rebuild_client().is_ok()
    }

    fn set_error(&self, e: &Error) {
        self.refusal.store(is_refusal(e), Relaxed);
        *self.last_error.lock() = Some(e.to_string());
    }

    /// What to show for this network: a network that is still being tried but keeps failing is
    /// reported as what is wrong with it, not as healthy.
    fn display_state(&self) -> NetState {
        let s = self.state();
        if s == NetState::Active {
            if self.capped() {
                return NetState::Capped;
            }
            if self.health.lock().consecutive_fail >= 3 {
                return if self.refusal.load(Relaxed) { NetState::Failed } else { NetState::Unreachable };
            }
        }
        s
    }

    fn note_ok(&self, ttfb: Option<Duration>) {
        let mut h = self.health.lock();
        h.ok = h.ok * 0.97 + 1.0;
        h.bad *= 0.97;
        h.consecutive_fail = 0;
        *self.last_error.lock() = None;
        if let Some(t) = ttfb {
            let ms = t.as_secs_f64() * 1000.0;
            h.lat_ewma = Some(h.lat_ewma.map_or(ms, |l| l * 0.8 + ms * 0.2));
        }
        drop(h);
        if self.probation.swap(false, Relaxed) {
            self.desired.store(self.base_streams, Relaxed);
        }
    }

    /// Returns the consecutive failure count.
    fn note_fail(&self) -> u32 {
        self.errors.fetch_add(1, Relaxed);
        let mut h = self.health.lock();
        h.bad = h.bad * 0.97 + 1.0;
        h.consecutive_fail += 1;
        h.consecutive_fail
    }

    pub fn health_score(&self) -> u8 {
        let h = self.health.lock();
        let total = h.ok + h.bad;
        let fail_rate = if total > 0.5 { h.bad / total } else { 0.0 };
        let lat_penalty = h.lat_ewma.map_or(0.0, |l| (l / 25.0).min(25.0));
        let q = (h.quarantines as f64 * 8.0).min(24.0);
        (100.0 - fail_rate * 70.0 - lat_penalty - q).clamp(0.0, 100.0) as u8
    }

    pub fn latency_ms(&self) -> Option<f64> {
        self.health.lock().lat_ewma
    }

    fn stream_speed(&self) -> f64 {
        let a = self.active.load(Relaxed).max(1) as f64;
        self.meter.speed() / a
    }
}

// ───────────────────────────── the run ─────────────────────────────

pub struct RunConfig {
    pub url: String,
    pub size: Option<u64>,
    pub ranges: bool,
    /// Strong ETag or Last-Modified, sent as `If-Range` so a changed file can't be spliced in.
    pub validator: Option<String>,
    pub headers: HeaderMap,
    pub staging: PathBuf,
    pub nets: Vec<NetPlan>,
    pub max_streams: u32,
    pub hedging: bool,
    pub saved: Option<SavedBlocks>,
    pub global_limiter: Arc<Limiter>,
    pub job_limiter: Arc<Limiter>,
    pub refresh: Option<Refresh>,
    pub stall_timeout: Duration,
    pub give_up_after: Duration,
    pub check_disk: bool,
    /// The server's host, port and addresses, so each network can pick an IP family it can use.
    pub target: Option<(String, u16, Vec<IpAddr>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Completed { size: u64 },
    Stopped,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Why {
    Running,
    Done,
    Stop,
    Fatal,
}

pub struct RunSnapshot {
    pub downloaded: u64,
    pub size: Option<u64>,
    pub speed: f64,
    pub eta: Option<u64>,
    pub blocks_total: u32,
    pub blocks_done: u32,
    pub nets: Vec<NetStat>,
}

pub struct Run {
    cfg_url: RwLock<String>,
    size: Option<u64>,
    ranges: bool,
    validator: Option<String>,
    headers: HeaderMap,
    file: Arc<File>,
    staging: PathBuf,
    table: Mutex<BlockTable>,
    nets: RwLock<Vec<Arc<NetRt>>>,
    target: Option<(String, u16, Vec<IpAddr>)>,
    max_streams: u32,
    meter: Meter,
    unique: AtomicU64,
    stop: CancellationToken,
    why: Mutex<Why>,
    fatal: Mutex<Option<Error>>,
    wake: Notify,
    strikes: AtomicU32,
    global: Arc<Limiter>,
    job: Arc<Limiter>,
    refresh: Option<Refresh>,
    refresh_gate: tokio::sync::Mutex<Option<Instant>>,
    hedging: bool,
    stall: Duration,
    give_up: Duration,
    workers: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    single_final: AtomicU64,
    net_ids: RwLock<Vec<String>>,
    streams: Mutex<Vec<Arc<StreamRt>>>,
    next_stream: AtomicU32,
}

/// What one connection is doing right now.
pub struct StreamRt {
    id: u32,
    net: usize,
    state: AtomicU8,
    /// 1-based block number, 0 = none.
    block: AtomicU32,
    meter: Meter,
}

const ST_IDLE: u8 = 0;
const ST_DOWNLOADING: u8 = 1;
const ST_RACING: u8 = 2;
const ST_RETRYING: u8 = 3;
const ST_PARKED: u8 = 4;

impl StreamRt {
    fn set(&self, state: u8, block: Option<usize>) {
        self.state.store(state, Relaxed);
        self.block.store(block.map_or(0, |b| b as u32 + 1), Relaxed);
    }
}

impl Run {
    pub fn new(cfg: RunConfig) -> Result<Arc<Run>> {
        if cfg.nets.is_empty() {
            return Err(Error::NoNetwork);
        }
        let size = cfg.size;
        let mut resumed = 0u64;
        let mut table: Option<BlockTable> = None;
        let net_ids: Vec<String> = cfg.nets.iter().map(|n| n.iface.id.clone()).collect();

        if let (Some(sz), true) = (size, cfg.ranges) {
            let existing = std::fs::metadata(&cfg.staging).map(|m| m.len()).ok();
            if let (Some(saved), Some(len)) = (&cfg.saved, existing) {
                if len == sz {
                    if let Some((mut t, done)) = BlockTable::restore(sz, saved) {
                        // The saved table speaks of the networks of the run that wrote it.
                        t.remap_networks(&saved.nets, &net_ids);
                        resumed = done;
                        table = Some(t);
                    }
                }
            }
            if table.is_none() {
                let total_streams: u32 = cfg.nets.iter().map(|n| n.streams).sum::<u32>().max(1);
                table = Some(BlockTable::new(sz, plan_block_size(sz, total_streams)));
            }
        } else if let Some(sz) = size {
            table = Some(BlockTable::new(sz, sz.max(1)));
        }

        let fresh = resumed == 0;
        if fresh && cfg.check_disk {
            if let (Some(sz), Some(dir)) = (size, cfg.staging.parent()) {
                let existing = std::fs::metadata(&cfg.staging).map(|m| m.len()).unwrap_or(0);
                if let Ok(free) = fs4::available_space(dir) {
                    if free + existing < sz {
                        return Err(Error::DiskFull { need: sz, have: free });
                    }
                }
            }
        }
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&cfg.staging)?;
        if fresh {
            file.set_len(size.unwrap_or(0))?;
        }

        let max = cfg.max_streams.max(MIN_STREAMS);
        let remaining_blocks = table.as_ref().map_or(1, |t| t.remaining_blocks().max(1)) as u32;
        let n_nets = net_ids.len() as u32;
        let nets: Vec<Arc<NetRt>> = cfg
            .nets
            .into_iter()
            .enumerate()
            .map(|(idx, p)| Arc::new(NetRt::build(idx, p, cfg.ranges, max, remaining_blocks, n_nets, cfg.target.clone())))
            .collect();
        let run = Arc::new(Run {
            cfg_url: RwLock::new(cfg.url),
            size,
            ranges: cfg.ranges,
            validator: cfg.validator,
            headers: cfg.headers,
            file: Arc::new(file),
            staging: cfg.staging,
            table: Mutex::new(table.unwrap_or_else(|| BlockTable::new(0, 1))),
            nets: RwLock::new(nets),
            target: cfg.target,
            max_streams: max,
            meter: Meter::default(),
            unique: AtomicU64::new(resumed),
            stop: CancellationToken::new(),
            why: Mutex::new(Why::Running),
            fatal: Mutex::new(None),
            wake: Notify::new(),
            strikes: AtomicU32::new(0),
            global: cfg.global_limiter,
            job: cfg.job_limiter,
            refresh: cfg.refresh,
            refresh_gate: tokio::sync::Mutex::new(None),
            hedging: cfg.hedging && cfg.ranges,
            stall: cfg.stall_timeout,
            give_up: cfg.give_up_after,
            workers: Mutex::new(Vec::new()),
            single_final: AtomicU64::new(0),
            net_ids: RwLock::new(net_ids),
            streams: Mutex::new(Vec::new()),
            next_stream: AtomicU32::new(1),
        });
        Ok(run)
    }

    fn nets(&self) -> Vec<Arc<NetRt>> {
        self.nets.read().clone()
    }

    /// Whether a network by this id is already part of the run.
    pub fn has_network(&self, id: &str) -> bool {
        self.net_ids.read().iter().any(|n| n == id)
    }

    /// A network that wasn't there when this download started has appeared. It joins the run
    /// switched **off**, so the user decides whether to put it to work — flipping it on makes it
    /// start pulling blocks at once. Returns false if it is already part of the run.
    pub fn add_network(self: &Arc<Self>, mut plan: NetPlan) -> bool {
        if self.has_network(&plan.iface.id) || self.net_ids.read().len() >= 250 {
            return false;
        }
        plan.enabled = false;
        let idx = self.nets.read().len();
        let remaining = self.table.lock().remaining_blocks().max(1) as u32;
        let id = plan.iface.id.clone();
        let rt = Arc::new(NetRt::build(idx, plan, self.ranges, self.max_streams, remaining, idx as u32 + 1, self.target.clone()));
        self.nets.write().push(rt);
        self.net_ids.write().push(id);
        self.wake.notify_waiters();
        true
    }

    // ───── control ─────

    pub fn stop(&self) {
        self.finish_with(Why::Stop);
    }

    fn finish_with(&self, why: Why) {
        let mut w = self.why.lock();
        if *w == Why::Running {
            *w = why;
        }
        drop(w);
        self.stop.cancel();
        self.wake.notify_waiters();
        for n in &self.nets() {
            n.nudge.notify_waiters();
        }
    }

    fn fail_fatal(&self, e: Error) {
        let mut f = self.fatal.lock();
        if f.is_none() {
            *f = Some(e);
        }
        drop(f);
        self.finish_with(Why::Fatal);
    }

    /// The computer woke from sleep or the network came back: sockets are probably dead.
    pub fn wake_up(&self) {
        for n in &self.nets() {
            n.bump_epoch();
            n.nudge.notify_waiters();
            n.health.lock().consecutive_fail = 0;
            if matches!(n.state(), NetState::Unreachable | NetState::Failed) {
                n.set_state(NetState::Active);
            }
        }
        self.wake.notify_waiters();
    }

    /// The set of networks changed: rebind what moved, park what vanished, revive what returned.
    pub fn networks_changed(&self, now: &[NetInterface]) {
        for n in &self.nets() {
            let cur = n.iface.read().clone();
            let off = n.user_off.load(Relaxed);
            match now.iter().find(|i| i.id == cur.id) {
                Some(i) if i.usable => {
                    let changed = i.ipv4 != cur.ipv4 || i.ipv6 != cur.ipv6;
                    if changed || n.state() == NetState::Offline {
                        *n.iface.write() = i.clone();
                        // Its addresses may have changed families: choose again.
                        let fam = match &n.target {
                            Some((_, _, ips)) => choose_family(i, ips),
                            None => choose_family(i, &[]),
                        };
                        n.family.store(fam.unwrap_or(0), Relaxed);
                        let _ = n.rebuild_client();
                        n.bump_epoch();
                    }
                    if n.state() == NetState::Offline && !off {
                        n.set_state(NetState::Active);
                    }
                    n.nudge.notify_waiters();
                }
                _ => {
                    if n.state() != NetState::Offline {
                        n.set_state(if off { NetState::Disabled } else { NetState::Offline });
                        n.bump_epoch();
                        self.table.lock().cancel_net(n.idx);
                    }
                }
            }
        }
        self.wake.notify_waiters();
    }

    /// Switch one network on or off for this download, while it runs. Off: its connections are
    /// cut and their blocks go back to the others. On: it rejoins at once.
    /// Returns whether a network by that id is part of this run.
    pub fn set_network_enabled(&self, id: &str, enabled: bool) -> bool {
        let nets = self.nets();
        let Some(n) = nets.iter().find(|n| n.iface.read().id == id) else { return false };
        n.user_off.store(!enabled, Relaxed);
        if enabled {
            let usable = n.iface.read().usable;
            n.set_state(if usable { NetState::Active } else { NetState::Offline });
            n.health.lock().consecutive_fail = 0;
            *n.last_error.lock() = None;
        } else {
            n.set_state(NetState::Disabled);
            n.bump_epoch();
            self.table.lock().cancel_net(n.idx);
        }
        n.nudge.notify_waiters();
        self.wake.notify_waiters();
        true
    }

    /// Whether any network could still carry this download (switched on and not capped).
    pub fn any_enabled(&self) -> bool {
        self.nets().iter().any(|n| !n.user_off.load(Relaxed))
    }

    // ───── observation ─────

    pub fn snapshot(&self) -> RunSnapshot {
        let (blocks_total, blocks_done) = {
            let t = self.table.lock();
            (t.blocks.len() as u32, t.done_blocks() as u32)
        };
        let downloaded = self.unique.load(Relaxed);
        let speed = self.meter.speed();
        let eta = self.size.and_then(|s| (speed > 1.0 && s > downloaded).then(|| ((s - downloaded) as f64 / speed).ceil() as u64));
        let nets = self
            .nets()
            .iter()
            .map(|n| {
                let i = n.iface.read();
                let state = n.display_state();
                NetStat {
                    id: i.id.clone(),
                    label: n.rule.label.clone().unwrap_or_else(|| i.label.clone()),
                    kind: i.kind,
                    bytes: n.meter.total(),
                    speed: n.meter.speed(),
                    streams: n.desired.load(Relaxed),
                    active: n.active.load(Relaxed),
                    latency_ms: n.latency_ms().unwrap_or(0.0),
                    health: n.health_score(),
                    errors: n.errors.load(Relaxed),
                    state,
                    error: if matches!(state, NetState::Active | NetState::Idle | NetState::Disabled) { None } else { n.last_error.lock().clone() },
                    enabled: !n.user_off.load(Relaxed),
                }
            })
            .collect();
        RunSnapshot { downloaded, size: self.size, speed, eta, blocks_total, blocks_done, nets }
    }

    pub fn block_map(&self) -> BlockMap {
        let t = self.table.lock();
        let (state, owner, fill) = t.map();
        BlockMap { block_size: t.block_size, networks: self.net_ids.read().clone(), state, owner, fill, split: t.split() }
    }

    /// Every connection and what it is doing — the per-connection table.
    pub fn streams_snapshot(&self) -> Vec<StreamStat> {
        self.streams
            .lock()
            .iter()
            .map(|s| StreamStat {
                id: s.id,
                net: self.net_ids.read().get(s.net).cloned().unwrap_or_default(),
                state: match s.state.load(Relaxed) {
                    ST_DOWNLOADING => "downloading",
                    ST_RACING => "racing",
                    ST_RETRYING => "retrying",
                    ST_PARKED => "parked",
                    _ => "idle",
                }
                .to_string(),
                block: Some(s.block.load(Relaxed)).filter(|b| *b > 0),
                bytes: s.meter.total(),
                speed: s.meter.speed(),
            })
            .collect()
    }

    pub fn saved(&self) -> SavedBlocks {
        self.table.lock().export(&self.net_ids.read())
    }

    pub fn downloaded(&self) -> u64 {
        self.unique.load(Relaxed)
    }

    // ───── driving ─────

    /// Run until every block is in, the run is stopped, or something unrecoverable happens.
    pub async fn drive(self: &Arc<Self>) -> Result<Outcome> {
        // Everything ranged is finished already (resuming a download that was complete).
        if self.ranges && self.table.lock().all_done() && self.size.is_some() {
            return Ok(Outcome::Completed { size: self.size.unwrap_or(0) });
        }
        if self.size == Some(0) {
            return Ok(Outcome::Completed { size: 0 });
        }

        let sampler = tokio::spawn(self.clone().sampler());
        let ramp = self.ranges.then(|| tokio::spawn(self.clone().ramp()));
        if self.ranges {
            self.spawn_needed_workers();
            // Workers are added lazily as the ramp grows; wait for the stop signal.
            self.stop.cancelled().await;
        } else {
            let me = self.clone();
            let h = tokio::spawn(async move { me.single_stream().await });
            self.workers.lock().push(h);
            self.stop.cancelled().await;
        }
        // Join everything so no stream is still writing when we return.
        loop {
            let h = self.workers.lock().pop();
            match h {
                Some(h) => {
                    let _ = h.await;
                }
                None => break,
            }
        }
        sampler.abort();
        if let Some(r) = ramp {
            r.abort();
        }

        let f = self.file.clone();
        let sync = tokio::task::spawn_blocking(move || f.sync_data()).await;
        if let Ok(Err(e)) = sync {
            tracing::warn!("sync failed: {e}");
        }
        if let Some(e) = self.fatal.lock().take() {
            return Err(e);
        }
        let why = *self.why.lock();
        match why {
            Why::Done => {
                let size = if self.ranges { self.size.unwrap_or(0) } else { self.single_final.load(Relaxed).max(self.size.unwrap_or(0)) };
                Ok(Outcome::Completed { size })
            }
            _ => Ok(Outcome::Stopped),
        }
    }

    fn spawn_needed_workers(self: &Arc<Self>) {
        for n in &self.nets() {
            loop {
                let want = n.desired.load(Relaxed).min(n.max);
                let have = n.spawned.load(Relaxed);
                if have >= want {
                    break;
                }
                n.spawned.store(have + 1, Relaxed);
                let (me, net) = (self.clone(), n.clone());
                let h = tokio::spawn(async move { me.worker(net, have as usize).await });
                self.workers.lock().push(h);
            }
        }
    }

    /// 4×/s: speeds, quarantine expiry, stall watchdog.
    async fn sampler(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        let mut last_total = 0u64;
        let mut last_move = Instant::now();
        loop {
            tick.tick().await;
            let now = Instant::now();
            self.meter.sample(now);
            for st in self.streams.lock().iter() {
                st.meter.sample(now);
            }
            for n in &self.nets() {
                n.meter.sample(now);
                let expired = n.until.lock().is_some_and(|u| now >= u);
                if expired && matches!(n.state(), NetState::Unreachable | NetState::Failed) {
                    // Probation: one stream feels out whether the network is back.
                    *n.until.lock() = None;
                    n.probation.store(true, Relaxed);
                    n.desired.store(1, Relaxed);
                    n.set_state(NetState::Active);
                    n.nudge.notify_waiters();
                }
            }
            let total = self.meter.total();
            if total != last_total {
                last_total = total;
                last_move = now;
            } else if now.duration_since(last_move) > self.give_up && !self.stop.is_cancelled() {
                self.fail_fatal(Error::msg(format!("No data arrived for {} minutes, so IDMX stopped. Check your connection, then resume — it will carry on from where it left off.", self.give_up.as_secs() / 60)));
                return;
            }
        }
    }

    /// Every second: find each network's sweet spot for stream count.
    async fn ramp(self: Arc<Self>) {
        struct R {
            last_speed: f64,
            settled_until: Option<Instant>,
        }
        let mut st: Vec<R> = Vec::new();
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut n_tick = 0u64;
        loop {
            tick.tick().await;
            n_tick += 1;
            // Networks can join mid-download; give each its own ramp state.
            while st.len() < self.nets().len() {
                st.push(R { last_speed: 0.0, settled_until: None });
            }
            let remaining = self.table.lock().remaining_blocks() as u32;
            let desired_total: u32 = self.nets().iter().filter(|n| n.runnable()).map(|n| n.desired.load(Relaxed)).sum();
            for (i, n) in self.nets().iter().enumerate() {
                if !n.runnable() || n.probation.load(Relaxed) {
                    continue;
                }
                let refused = n.refused.swap(0, Relaxed);
                let desired = n.desired.load(Relaxed);
                if refused > 0 {
                    // The server (or a middlebox) is turning connections away: stay under it.
                    let active = n.active.load(Relaxed).max(MIN_STREAMS);
                    n.desired.store(desired.min(active * 3 / 4).max(MIN_STREAMS), Relaxed);
                    st[i].settled_until = Some(Instant::now() + Duration::from_secs(60));
                    continue;
                }
                if n_tick % 2 != 0 || st[i].settled_until.is_some_and(|u| Instant::now() < u) {
                    continue;
                }
                let speed = n.meter.speed();
                let all_receiving = n.active.load(Relaxed) + 1 >= desired;
                let room = remaining > desired_total;
                if all_receiving && room && desired < n.max {
                    if st[i].last_speed == 0.0 || speed >= st[i].last_speed * 1.08 {
                        st[i].last_speed = speed;
                        n.desired.store((desired * 2).min(n.max).min(desired + remaining.saturating_sub(desired_total).max(1)), Relaxed);
                    } else {
                        // More streams stopped helping: the link is full.
                        st[i].settled_until = Some(Instant::now() + Duration::from_secs(20));
                    }
                }
            }
            self.spawn_needed_workers();
        }
    }

    // ───── a stream ─────

    async fn worker(self: Arc<Self>, net: Arc<NetRt>, sid: usize) {
        let mut failures = 0u32;
        let me = Arc::new(StreamRt { id: self.next_stream.fetch_add(1, Relaxed), net: net.idx, state: AtomicU8::new(ST_PARKED), block: AtomicU32::new(0), meter: Meter::default() });
        self.streams.lock().push(me.clone());
        loop {
            if self.stop.is_cancelled() {
                return;
            }
            if !net.runnable() || sid as u32 >= net.desired.load(Relaxed) {
                me.set(ST_PARKED, None);
                self.park(&net).await;
                continue;
            }
            let claim = {
                let now = Instant::now();
                let other_free = self.nets().iter().any(|o| o.idx != net.idx && o.runnable());
                let hedge = self.hedging.then_some(HedgePolicy { after: Duration::from_secs(2), startup: Duration::from_millis(300) });
                let my_speed = net.stream_speed().max(self.meter.speed() / self.nets().iter().map(|n| n.active.load(Relaxed)).sum::<u32>().max(1) as f64 * 0.5);
                let mut t = self.table.lock();
                if t.all_done() {
                    drop(t);
                    self.finish_with(Why::Done);
                    return;
                }
                t.claim(net.idx, other_free, hedge, my_speed, now)
            };
            let Some(claim) = claim else {
                // Nothing to take right now: wait for a block to come back or for a hedge to ripen.
                me.set(ST_IDLE, None);
                tokio::select! {
                    _ = self.stop.cancelled() => return,
                    _ = self.wake.notified() => {}
                    _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                }
                continue;
            };

            net.active.fetch_add(1, Relaxed);
            me.set(if claim.kind == Kind::Hedge { ST_RACING } else { ST_DOWNLOADING }, Some(claim.idx));
            let result = self.fetch(&net, &claim, &me).await;
            net.active.fetch_sub(1, Relaxed);
            me.set(ST_IDLE, None);

            match result {
                Fetch::Done => {
                    failures = 0;
                    self.strikes.store(0, Relaxed);
                    let all = {
                        let mut t = self.table.lock();
                        t.finish(claim.idx, claim.attempt, net.idx);
                        t.all_done()
                    };
                    self.wake.notify_waiters();
                    if all {
                        self.finish_with(Why::Done);
                        return;
                    }
                }
                Fetch::Aborted { delivered } => {
                    self.table.lock().release(claim.idx, claim.attempt, delivered);
                    self.wake.notify_waiters();
                }
                Fetch::Failed { err, delivered } => {
                    self.table.lock().release(claim.idx, claim.attempt, delivered);
                    self.wake.notify_waiters();
                    let fatal = matches!(err, Error::RemoteChanged | Error::DiskFull { .. })
                        || (!err.is_transient() && self.strikes.fetch_add(1, Relaxed) + 1 >= STRIKES_FATAL);
                    if matches!(err, Error::Io(ref e) if e.kind() == std::io::ErrorKind::StorageFull) {
                        self.fail_fatal(err);
                        return;
                    }
                    if fatal {
                        self.fail_fatal(err);
                        return;
                    }
                    failures += 1;
                    let consecutive = net.note_fail();
                    net.set_error(&err);
                    tracing::debug!(net = %net.iface.read().id, "attempt failed: {err}");
                    self.maybe_quarantine(&net, consecutive);
                    // Couldn't even connect over this IP version: try the other one right away
                    // (if this network and the server both have it) instead of waiting.
                    if could_not_connect(&err) && net.switch_family() {
                        failures = 0;
                        tracing::info!(net = %net.iface.read().id, family = net.family.load(Relaxed), "switched IP version");
                        continue;
                    }
                    me.set(ST_RETRYING, None);
                    let wait = backoff(failures);
                    tokio::select! {
                        _ = self.stop.cancelled() => return,
                        _ = net.nudge.notified() => { failures = 0; }
                        _ = tokio::time::sleep(wait) => {}
                    }
                }
            }
        }
    }

    async fn park(&self, net: &NetRt) {
        tokio::select! {
            _ = self.stop.cancelled() => {}
            _ = net.nudge.notified() => {}
            _ = tokio::time::sleep(Duration::from_millis(300)) => {}
        }
    }

    /// Pull a network out of rotation after repeated failures — but never the last one standing.
    fn maybe_quarantine(&self, net: &NetRt, consecutive: u32) {
        if consecutive < QUARANTINE_AFTER || net.state() != NetState::Active {
            return;
        }
        let others = self.nets().iter().any(|o| o.idx != net.idx && o.state() == NetState::Active);
        if !others {
            return;
        }
        let mut h = net.health.lock();
        h.quarantines += 1;
        let secs = (15u64 << (h.quarantines - 1).min(5)).min(300);
        h.consecutive_fail = 0;
        drop(h);
        *net.until.lock() = Some(Instant::now() + Duration::from_secs(secs));
        net.set_state(if net.refusal.load(Relaxed) { NetState::Failed } else { NetState::Unreachable });
        net.bump_epoch();
        self.table.lock().cancel_net(net.idx);
        tracing::info!(net = %net.iface.read().id, secs, "network quarantined");
    }

    fn url(&self) -> String {
        self.cfg_url.read().clone()
    }

    /// Signed download URLs expire. Ask the engine for a fresh one — at most every few seconds.
    async fn refresh_url(&self, stale: &str) -> bool {
        let Some(f) = &self.refresh else { return false };
        let mut gate = self.refresh_gate.lock().await;
        // Someone else already refreshed while we waited for the lock.
        if self.url() != stale {
            return true;
        }
        if gate.is_some_and(|t| t.elapsed() < Duration::from_secs(5)) {
            return false;
        }
        *gate = Some(Instant::now());
        match f().await {
            Ok(u) => {
                *self.cfg_url.write() = u;
                true
            }
            Err(e) => {
                tracing::debug!("refresh failed: {e}");
                false
            }
        }
    }

    #[allow(unused_assignments)]
    async fn fetch(&self, net: &Arc<NetRt>, claim: &Claim, st: &StreamRt) -> Fetch {
        let client = net.client();
        let url = self.url();
        let mut req = ranged(&client, &url, &self.headers, claim.start, Some(claim.end));
        if let Some(v) = &self.validator {
            req = req.header(IF_RANGE, v.as_str());
        }
        let mut epoch = net.epoch.subscribe();
        epoch.borrow_and_update();
        let started = Instant::now();
        let sent = tokio::select! {
            biased;
            _ = self.stop.cancelled() => return Fetch::Aborted { delivered: false },
            _ = claim.token.cancelled() => return Fetch::Aborted { delivered: false },
            _ = epoch.changed() => return Fetch::Aborted { delivered: false },
            r = tokio::time::timeout(CONNECT_AND_TTFB, req.send()) => r,
        };
        let resp = match sent {
            Err(_) => return fail(Error::msg(SLOW_SERVER), false),
            Ok(Err(e)) => return fail(Error::Net(e), false),
            Ok(Ok(r)) => r,
        };
        let ttfb = started.elapsed();
        let status = resp.status();
        match status {
            StatusCode::PARTIAL_CONTENT => {
                let ok = resp
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(parse_content_range)
                    .is_some_and(|(a, _, _)| a == claim.start);
                if !ok {
                    return fail(Error::msg("The server sent the wrong part of the file. IDMX will retry that piece."), false);
                }
            }
            // We asked for a range and were sent the whole thing (or a different file).
            StatusCode::OK => return fail(Error::RemoteChanged, false),
            StatusCode::RANGE_NOT_SATISFIABLE => return fail(Error::RemoteChanged, false),
            s if matches!(s.as_u16(), 403 | 410) => {
                if self.refresh_url(&url).await {
                    return fail(Error::Http(s.as_u16()), false);
                }
                net.refused.fetch_add(1, Relaxed);
                return fail(Error::Http(s.as_u16()), false);
            }
            s => {
                if matches!(s.as_u16(), 429 | 503) {
                    net.refused.fetch_add(1, Relaxed);
                }
                return fail(Error::Http(s.as_u16()), false);
            }
        }

        let expected = claim.end - claim.start + 1;
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::with_capacity(FLUSH_AT + (64 << 10));
        let mut pos = claim.start;
        let mut got = 0u64;
        let mut flushed_any = false;
        let mut first = true;

        macro_rules! flush {
            () => {
                if !buf.is_empty() {
                    let data = std::mem::replace(&mut buf, Vec::with_capacity(FLUSH_AT + (64 << 10)));
                    let n = data.len() as u64;
                    let (f, off) = (self.file.clone(), pos);
                    match tokio::task::spawn_blocking(move || write_all_at(&f, &data, off)).await {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => return Fetch::Failed { err: Error::Io(e), delivered: flushed_any },
                        Err(e) => return Fetch::Failed { err: Error::msg(e.to_string()), delivered: flushed_any },
                    }
                    pos += n;
                    flushed_any = true;
                    let new = self.table.lock().progress(claim.idx, claim.attempt, n);
                    self.unique.fetch_add(new, Relaxed);
                }
            };
        }

        loop {
            let next = tokio::select! {
                biased;
                _ = self.stop.cancelled() => { flush!(); return Fetch::Aborted { delivered: flushed_any }; }
                _ = claim.token.cancelled() => { flush!(); return Fetch::Aborted { delivered: flushed_any }; }
                _ = epoch.changed() => { flush!(); return Fetch::Aborted { delivered: flushed_any }; }
                r = tokio::time::timeout(self.stall, stream.next()) => r,
            };
            let chunk = match next {
                Err(_) => {
                    flush!();
                    return fail(Error::msg("The connection stalled — no data for 20 seconds. IDMX will retry."), flushed_any);
                }
                Ok(None) => break,
                Ok(Some(Err(e))) => {
                    flush!();
                    return fail(Error::Net(e), flushed_any);
                }
                Ok(Some(Ok(c))) => c,
            };
            if first {
                first = false;
                net.note_ok(Some(ttfb));
            }
            let take = (chunk.len() as u64).min(expected - got) as usize;
            if take == 0 {
                break;
            }
            let n = take as u64;
            // Pace ourselves: global, per-network and per-job ceilings.
            let limiters = [&*self.global, &*net.limiter, &*self.job];
            tokio::select! {
                _ = acquire_all(&limiters, n) => {}
                _ = self.stop.cancelled() => { flush!(); return Fetch::Aborted { delivered: flushed_any }; }
                _ = claim.token.cancelled() => { flush!(); return Fetch::Aborted { delivered: flushed_any }; }
            }
            self.meter.add(n);
            net.meter.add(n);
            st.meter.add(n);
            net.usage.fetch_add(n, Relaxed);
            buf.extend_from_slice(&chunk[..take]);
            got += n;
            if buf.len() >= FLUSH_AT {
                flush!();
            }
            if got >= expected {
                break;
            }
            if net.capped() {
                flush!();
                return Fetch::Aborted { delivered: flushed_any };
            }
        }
        flush!();
        if got < expected {
            return fail(Error::msg(format!("The server closed the connection {} early. IDMX will retry.", crate::util::human_bytes(expected - got))), flushed_any);
        }
        Fetch::Done
    }

    // ───── a server that can't do ranges ─────

    /// One plain GET, streamed straight to disk. Not resumable, but it works anywhere.
    ///
    /// It runs over a single network — the first one that is switched on and up. Switch that one
    /// off (or lose it) and the download starts again over the next; with none left it waits for
    /// one to be switched on or to come back.
    async fn single_stream(self: Arc<Self>) {
        let mut failures = 0u32;
        loop {
            if self.stop.is_cancelled() {
                return;
            }
            let Some(net) = self.nets().into_iter().find(|n| n.runnable()) else {
                tokio::select! {
                    _ = self.stop.cancelled() => return,
                    _ = self.wake.notified() => {}
                    _ = tokio::time::sleep(Duration::from_millis(300)) => {}
                }
                continue;
            };
            match self.single_attempt(&net).await {
                Ok(written) => {
                    self.single_final.store(written, Relaxed);
                    let (f, w) = (self.file.clone(), written);
                    let _ = tokio::task::spawn_blocking(move || f.set_len(w)).await;
                    self.finish_with(Why::Done);
                    return;
                }
                // Stopped, or the network was switched off / went away: pick again.
                Err(Fetch::Aborted { .. }) => continue,
                Err(Fetch::Failed { err, .. }) => {
                    failures += 1;
                    if !err.is_transient() || failures > 8 {
                        self.fail_fatal(err);
                        return;
                    }
                    net.note_fail();
                    net.set_error(&err);
                    tokio::select! {
                        _ = self.stop.cancelled() => return,
                        _ = net.nudge.notified() => { failures = 0; }
                        _ = tokio::time::sleep(backoff(failures)) => {}
                    }
                }
                Err(Fetch::Done) => return,
            }
        }
    }

    async fn single_attempt(&self, net: &Arc<NetRt>) -> std::result::Result<u64, Fetch> {
        // Restart from the top: without ranges there is nothing to resume.
        self.unique.store(0, Relaxed);
        let mut epoch = net.epoch.subscribe();
        epoch.borrow_and_update();
        let client = net.client();
        let req = client.get(self.url()).headers(self.headers.clone());
        let started = Instant::now();
        let resp = tokio::select! {
            biased;
            _ = self.stop.cancelled() => return Err(Fetch::Aborted { delivered: false }),
            _ = epoch.changed() => return Err(Fetch::Aborted { delivered: false }),
            r = tokio::time::timeout(CONNECT_AND_TTFB, req.send()) => match r {
                Err(_) => return Err(Fetch::Failed { err: Error::msg(SLOW_SERVER), delivered: false }),
                Ok(Err(e)) => return Err(Fetch::Failed { err: Error::Net(e), delivered: false }),
                Ok(Ok(r)) => r,
            }
        };
        if !resp.status().is_success() {
            return Err(Fetch::Failed { err: Error::Http(resp.status().as_u16()), delivered: false });
        }
        net.note_ok(Some(started.elapsed()));
        // The one connection, for the connections table.
        let st = Arc::new(StreamRt { id: self.next_stream.fetch_add(1, Relaxed), net: net.idx, state: AtomicU8::new(ST_DOWNLOADING), block: AtomicU32::new(1), meter: Meter::default() });
        {
            let mut list = self.streams.lock();
            list.clear();
            list.push(st.clone());
        }
        net.active.fetch_add(1, Relaxed);
        let attempt = {
            let mut t = self.table.lock();
            if t.blocks.is_empty() { None } else { t.claim(net.idx, false, None, 0.0, Instant::now()).map(|c| c.attempt) }
        };
        let out = self.single_body(net, resp, &mut epoch, &st, attempt).await;
        net.active.fetch_sub(1, Relaxed);
        st.set(ST_IDLE, None);
        if out.is_err() {
            if let Some(a) = attempt {
                self.table.lock().release(0, a, true);
            }
        }
        out
    }

    async fn single_body(&self, net: &Arc<NetRt>, resp: reqwest::Response, epoch: &mut watch::Receiver<u64>, st: &StreamRt, attempt: Option<u64>) -> std::result::Result<u64, Fetch> {
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::with_capacity(FLUSH_AT + (64 << 10));
        let mut pos = 0u64;
        loop {
            let next = tokio::select! {
                biased;
                _ = self.stop.cancelled() => return Err(Fetch::Aborted { delivered: pos > 0 }),
                _ = epoch.changed() => return Err(Fetch::Aborted { delivered: pos > 0 }),
                r = tokio::time::timeout(self.stall, stream.next()) => r,
            };
            let chunk = match next {
                Err(_) => return Err(Fetch::Failed { err: Error::msg("The connection stalled — no data for 20 seconds. IDMX will retry."), delivered: pos > 0 }),
                Ok(None) => break,
                Ok(Some(Err(e))) => return Err(Fetch::Failed { err: Error::Net(e), delivered: pos > 0 }),
                Ok(Some(Ok(c))) => c,
            };
            let n = chunk.len() as u64;
            acquire_all(&[&*self.global, &*net.limiter, &*self.job], n).await;
            self.meter.add(n);
            net.meter.add(n);
            st.meter.add(n);
            net.usage.fetch_add(n, Relaxed);
            buf.extend_from_slice(&chunk);
            if buf.len() >= FLUSH_AT {
                let data = std::mem::take(&mut buf);
                let (f, off, len) = (self.file.clone(), pos, data.len() as u64);
                tokio::task::spawn_blocking(move || write_all_at(&f, &data, off))
                    .await
                    .map_err(|e| Fetch::Failed { err: Error::msg(e.to_string()), delivered: pos > 0 })?
                    .map_err(|e| Fetch::Failed { err: Error::Io(e), delivered: pos > 0 })?;
                pos += len;
                self.unique.store(pos, Relaxed);
                if let Some(a) = attempt {
                    self.table.lock().progress(0, a, len);
                }
            }
        }
        if !buf.is_empty() {
            let (f, off, len) = (self.file.clone(), pos, buf.len() as u64);
            tokio::task::spawn_blocking(move || write_all_at(&f, &buf, off))
                .await
                .map_err(|e| Fetch::Failed { err: Error::msg(e.to_string()), delivered: pos > 0 })?
                .map_err(|e| Fetch::Failed { err: Error::Io(e), delivered: pos > 0 })?;
            pos += len;
            self.unique.store(pos, Relaxed);
            if let Some(a) = attempt {
                self.table.lock().progress(0, a, len);
            }
        }
        if let Some(a) = attempt {
            self.table.lock().finish(0, a, net.idx);
        }
        Ok(pos)
    }

    pub fn staging(&self) -> &PathBuf {
        &self.staging
    }

    pub fn owner_label(&self, owner: u8) -> Option<String> {
        if owner == NO_OWNER { None } else { self.net_ids.read().get(owner as usize).cloned() }
    }
}

enum Fetch {
    Done,
    /// Called off (stop, network change, someone else won): not a failure.
    Aborted { delivered: bool },
    Failed { err: Error, delivered: bool },
}

/// The failure was in making the connection (refused, unreachable, timed out) rather than in
/// what the server said — the kind a different IP version might get around.
fn could_not_connect(e: &Error) -> bool {
    match e {
        Error::Net(e) => e.is_connect() || e.is_timeout(),
        Error::Msg(m) => m == SLOW_SERVER,
        _ => false,
    }
}

fn fail(err: Error, delivered: bool) -> Fetch {
    Fetch::Failed { err, delivered }
}

/// 1 s, 2 s, 4 s … capped at 15 s, with jitter so streams don't retry in lockstep.
fn backoff(failures: u32) -> Duration {
    let base = (1u64 << failures.saturating_sub(1).min(4)).min(15);
    let jitter = rand::rng().random_range(0.6..1.0);
    Duration::from_secs_f64(base as f64 * jitter)
}

impl NetPlan {
    fn rule_streams_cap(&self, global_max: u32) -> u32 {
        self.rule.streams.map(|s| s.clamp(MIN_STREAMS, global_max)).unwrap_or(global_max)
    }
}

#[allow(dead_code)]
fn _kind_is_used(k: Kind) -> bool {
    k == Kind::Hedge
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        assert!(backoff(1) <= Duration::from_secs(1));
        assert!(backoff(3) <= Duration::from_secs(4));
        assert!(backoff(30) <= Duration::from_secs(15));
        assert!(backoff(30) >= Duration::from_secs(8));
    }

    #[test]
    fn meter_smooths() {
        let m = Meter::default();
        let t0 = Instant::now();
        m.sample(t0);
        m.add(1_000_000);
        m.sample(t0 + Duration::from_secs(1));
        assert!(m.speed() > 200_000.0 && m.speed() <= 1_000_000.0);
    }
}
