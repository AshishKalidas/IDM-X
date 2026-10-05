mod common;

use common::*;
use idmx_core::http::base_headers;
use idmx_core::limiter::Limiter;
use idmx_core::run::{NetPlan, Outcome, Run, RunConfig};
use idmx_core::types::{IfKind, InterfaceRule, JobOptions, NetInterface};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn loopback(id: &str) -> NetPlan {
    NetPlan {
        iface: NetInterface {
            id: id.into(),
            label: id.into(),
            kind: IfKind::Ethernet,
            ipv4: vec!["127.0.0.1".into()],
            ipv6: vec![],
            gateway: None,
            subnets: vec![],
            mac: None,
            up: true,
            link_mbps: None,
            usable: true,
        },
        rule: InterfaceRule::default(),
        streams: 4,
        limiter: Arc::new(Limiter::new(None)),
        usage: Arc::new(AtomicU64::new(0)),
        cap: None,
        bind_device: false,
        proxy: None,
        enabled: true,
    }
}

fn cfg(url: String, size: u64, dir: &std::path::Path, nets: Vec<NetPlan>) -> RunConfig {
    RunConfig {
        url,
        size: Some(size),
        ranges: true,
        validator: Some("\"v1\"".into()),
        headers: base_headers(&JobOptions::default(), None),
        staging: dir.join("out.bin.idmx"),
        nets,
        max_streams: 16,
        hedging: true,
        saved: None,
        global_limiter: Arc::new(Limiter::new(None)),
        job_limiter: Arc::new(Limiter::new(None)),
        refresh: None,
        stall_timeout: Duration::from_secs(5),
        give_up_after: Duration::from_secs(30),
        check_disk: true,
        target: None,
    }
}

fn assert_file_matches(path: &std::path::Path, size: u64) {
    let got = std::fs::read(path).unwrap();
    assert_eq!(got.len() as u64, size, "length");
    let want = expected(size);
    if got != want {
        let first = got.iter().zip(&want).position(|(a, b)| a != b).unwrap();
        panic!("content differs, first bad byte at {first}");
    }
}

#[tokio::test]
async fn downloads_a_file_across_two_networks_byte_for_byte() {
    let size = 24 << 20;
    let o = start(Config { size, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let run = Run::new(cfg(o.url("f.bin"), size, dir.path(), vec![loopback("netA"), loopback("netB")])).unwrap();
    let out = run.drive().await.unwrap();
    assert_eq!(out, Outcome::Completed { size });
    assert_file_matches(&dir.path().join("out.bin.idmx"), size);

    let snap = run.snapshot();
    assert_eq!(snap.downloaded, size);
    assert_eq!(snap.blocks_done, snap.blocks_total);
    // Both networks pulled real weight.
    for n in &snap.nets {
        assert!(n.bytes > size / 10, "{} only moved {}", n.id, n.bytes);
    }
    let map = run.block_map();
    assert!(map.state.iter().all(|s| *s == 2));
    assert!(map.owner.iter().all(|o| *o < 2));
}

#[tokio::test]
async fn survives_dropped_connections() {
    let size = 12 << 20;
    let o = start(Config { size, drop_first: 8, drop_after_bytes: 200_000, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let run = Run::new(cfg(o.url("f.bin"), size, dir.path(), vec![loopback("netA")])).unwrap();
    let out = tokio::time::timeout(Duration::from_secs(60), run.drive()).await.expect("hung").unwrap();
    assert_eq!(out, Outcome::Completed { size });
    assert_file_matches(&dir.path().join("out.bin.idmx"), size);
    assert!(o.requests.load(std::sync::atomic::Ordering::Relaxed) > 8);
}

#[tokio::test]
async fn pause_then_resume_does_not_refetch_everything() {
    let size = 16 << 20;
    let o = start(Config { size, per_conn_bps: 1_000_000, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();

    let run = Run::new(cfg(o.url("f.bin"), size, dir.path(), vec![loopback("netA")])).unwrap();
    let r2 = run.clone();
    let handle = tokio::spawn(async move { r2.drive().await });
    tokio::time::sleep(Duration::from_millis(1500)).await;
    run.stop();
    assert_eq!(handle.await.unwrap().unwrap(), Outcome::Stopped);
    let first = run.snapshot().downloaded;
    assert!(first > 0 && first < size, "paused at {first}");
    let saved = run.saved();
    let first_net_bytes = run.snapshot().nets[0].bytes;

    // Resume with a fresh Run object (as after an app restart).
    let mut c = cfg(o.url("f.bin"), size, dir.path(), vec![loopback("netA")]);
    c.saved = Some(saved);
    let run2 = Run::new(c).unwrap();
    assert!(run2.downloaded() > 0, "resume should start from saved progress");
    let out = tokio::time::timeout(Duration::from_secs(60), run2.drive()).await.expect("hung").unwrap();
    assert_eq!(out, Outcome::Completed { size });
    assert_file_matches(&dir.path().join("out.bin.idmx"), size);

    // Second leg fetched roughly what was missing — not the whole file again.
    let second = run2.snapshot().nets[0].bytes;
    assert!(
        first_net_bytes + second < size + (size / 4),
        "fetched {} + {} for a {} byte file",
        first_net_bytes,
        second,
        size
    );
}

#[tokio::test]
async fn refuses_to_splice_a_changed_file() {
    let size = 8 << 20;
    let o = start(Config { size, per_conn_bps: 3_000_000, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let run = Run::new(cfg(o.url("f.bin"), size, dir.path(), vec![loopback("netA")])).unwrap();
    let r2 = run.clone();
    let h = tokio::spawn(async move { r2.drive().await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    // The file is replaced on the server mid-download: new ETag → If-Range fails → 200.
    o.cfg.lock().etag = "\"v2\"".into();
    let res = tokio::time::timeout(Duration::from_secs(30), h).await.expect("hung").unwrap();
    assert!(matches!(res, Err(idmx_core::error::Error::RemoteChanged)), "{res:?}");
}

#[tokio::test]
async fn honours_speed_limits() {
    let size = 6 << 20;
    let o = start(Config { size, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let mut c = cfg(o.url("f.bin"), size, dir.path(), vec![loopback("netA")]);
    c.job_limiter = Arc::new(Limiter::new(Some(3_000_000)));
    let t = Instant::now();
    let run = Run::new(c).unwrap();
    run.drive().await.unwrap();
    let secs = t.elapsed().as_secs_f64();
    // 6 MB at 3 MB/s, minus the initial burst allowance.
    assert!(secs > 1.2, "finished in {secs}s — limiter not applied");
    assert!(secs < 8.0, "took {secs}s");
    assert_file_matches(&dir.path().join("out.bin.idmx"), size);
}

#[tokio::test]
async fn falls_back_to_a_single_stream_without_range_support() {
    let size = 5 << 20;
    let o = start(Config { size, ranges: false, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let mut c = cfg(o.url("f.bin"), size, dir.path(), vec![loopback("netA")]);
    c.ranges = false;
    c.validator = None;
    let run = Run::new(c).unwrap();
    let out = tokio::time::timeout(Duration::from_secs(30), run.drive()).await.expect("hung").unwrap();
    assert_eq!(out, Outcome::Completed { size });
    assert_file_matches(&dir.path().join("out.bin.idmx"), size);
}

#[tokio::test]
async fn backs_off_when_the_server_limits_connections() {
    let size = 24 << 20;
    // Only 3 concurrent connections are allowed; the rest get 503.
    let o = start(Config { size, max_conns: 3, per_conn_bps: 6_000_000, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let mut p = loopback("netA");
    p.streams = 8;
    let run = Run::new(cfg(o.url("f.bin"), size, dir.path(), vec![p])).unwrap();
    let out = tokio::time::timeout(Duration::from_secs(90), run.drive()).await.expect("hung").unwrap();
    assert_eq!(out, Outcome::Completed { size });
    assert_file_matches(&dir.path().join("out.bin.idmx"), size);
}

#[tokio::test]
async fn data_cap_parks_a_network() {
    let size = 12 << 20;
    let o = start(Config { size, per_conn_bps: 8_000_000, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    // Network A is capped at ~2 MB; B has no cap and must finish the job.
    let mut a = loopback("capped");
    a.cap = Some(2 << 20);
    let b = loopback("free");
    let run = Run::new(cfg(o.url("f.bin"), size, dir.path(), vec![a, b])).unwrap();
    let out = tokio::time::timeout(Duration::from_secs(60), run.drive()).await.expect("hung").unwrap();
    assert_eq!(out, Outcome::Completed { size });
    assert_file_matches(&dir.path().join("out.bin.idmx"), size);
    let snap = run.snapshot();
    let capped = snap.nets.iter().find(|n| n.id == "capped").unwrap();
    // It overshoots by at most the data already in flight when the cap hit.
    assert!(capped.bytes < (2 << 20) + (6 << 20), "capped net moved {}", capped.bytes);
}


fn two(a: &str, b: &str) -> Vec<NetPlan> {
    vec![loopback(a), loopback(b)]
}

/// Two networks that each can only move `bps` — a deterministic stand-in for real links.
fn two_limited(a: &str, b: &str, bps: u64) -> Vec<NetPlan> {
    let mut v = two(a, b);
    for p in &mut v {
        p.limiter = Arc::new(Limiter::new(Some(bps)));
    }
    v
}

fn bytes_of(run: &Run, id: &str) -> u64 {
    run.snapshot().nets.iter().find(|n| n.id == id).map(|n| n.bytes).unwrap()
}

#[tokio::test]
async fn a_network_switched_off_midway_hands_its_work_to_the_others_and_can_return() {
    let size = 60 << 20;
    let o = start(Config { size, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let run = Run::new(cfg(o.url("f.bin"), size, dir.path(), two_limited("a", "b", 2_000_000))).unwrap();
    let r2 = run.clone();
    let h = tokio::spawn(async move { r2.drive().await });

    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(bytes_of(&run, "b") > 0, "b should be working before it is switched off");
    assert!(run.set_network_enabled("b", false));
    tokio::time::sleep(Duration::from_millis(500)).await; // let in-flight reads wind down
    let snap = run.snapshot();
    let b = snap.nets.iter().find(|n| n.id == "b").unwrap();
    assert_eq!((b.state, b.enabled), (idmx_core::types::NetState::Disabled, false));
    let frozen = b.bytes;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(bytes_of(&run, "b"), frozen, "a switched-off network must stop transferring");
    assert!(bytes_of(&run, "a") > 0 && !h.is_finished());

    // Bring it back: it rejoins and moves more bytes.
    assert!(run.set_network_enabled("b", true));
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(bytes_of(&run, "b") > frozen, "re-enabled network should rejoin");
    assert!(!run.set_network_enabled("nonexistent", true));

    let out = tokio::time::timeout(Duration::from_secs(90), h).await.expect("hung").unwrap().unwrap();
    assert_eq!(out, Outcome::Completed { size });
    assert_file_matches(&dir.path().join("out.bin.idmx"), size);
}

#[tokio::test]
async fn a_network_that_starts_off_does_nothing_until_switched_on() {
    let size = 60 << 20;
    let o = start(Config { size, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let mut nets = two_limited("a", "b", 2_000_000);
    nets[1].enabled = false;
    let run = Run::new(cfg(o.url("f.bin"), size, dir.path(), nets)).unwrap();
    let r2 = run.clone();
    let h = tokio::spawn(async move { r2.drive().await });
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(bytes_of(&run, "b"), 0);
    assert!(run.any_enabled());
    run.set_network_enabled("b", true);
    tokio::time::sleep(Duration::from_millis(2000)).await;
    assert!(bytes_of(&run, "b") > 0);
    // Switching everything off is visible to the caller, which pauses the download.
    run.set_network_enabled("a", false);
    run.set_network_enabled("b", false);
    assert!(!run.any_enabled());
    run.stop();
    assert_eq!(h.await.unwrap().unwrap(), Outcome::Stopped);
}

#[tokio::test]
async fn falls_back_to_ipv6_when_the_server_only_listens_there() {
    // The origin is on [::1] only. "localhost" resolves to both families; the network has both.
    let size = 6 << 20;
    let o = start_on(Config { size, ..Default::default() }, "[::1]:0".to_string()).await;
    let port = o.addr.port();
    let dir = tempfile::tempdir().unwrap();
    let mut p = loopback("dual");
    p.iface.ipv6 = vec!["::1".into()];
    let mut c = cfg(format!("http://localhost:{port}/f.bin"), size, dir.path(), vec![p]);
    c.target = Some(("localhost".into(), port, vec!["127.0.0.1".parse().unwrap(), "::1".parse().unwrap()]));
    let run = Run::new(c).unwrap();
    let out = tokio::time::timeout(Duration::from_secs(30), run.drive()).await.expect("hung").unwrap();
    assert_eq!(out, Outcome::Completed { size });
    assert_file_matches(&dir.path().join("out.bin.idmx"), size);
}

#[tokio::test]
async fn an_ipv6_only_server_over_an_ipv4_only_network_says_why() {
    use idmx_core::types::NetState;
    let dir = tempfile::tempdir().unwrap();
    let mut c = cfg("http://nowhere.invalid/f.bin".into(), 5 << 20, dir.path(), vec![loopback("v4only")]);
    c.target = Some(("nowhere.invalid".into(), 80, vec!["2001:db8::1".parse().unwrap()]));
    let run = Run::new(c).unwrap();
    let n = &run.snapshot().nets[0];
    assert_eq!(n.state, NetState::Unreachable);
    assert!(n.error.as_deref().unwrap_or("").contains("IP version"), "{:?}", n.error);
}

#[tokio::test]
async fn an_unreachable_network_is_reported_with_its_error_while_the_others_finish() {
    use idmx_core::types::NetState;
    let size = 24 << 20;
    let o = start(Config { size, per_conn_bps: 2_500_000, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let mut bad = loopback("ghost");
    // An address this machine doesn't own can't be bound: every connection from it fails.
    bad.iface.ipv4 = vec!["203.0.113.77".into()];
    let run = Run::new(cfg(o.url("f.bin"), size, dir.path(), vec![loopback("good"), bad])).unwrap();
    let r2 = run.clone();
    let h = tokio::spawn(async move { r2.drive().await });
    // Failures back off 1 s, 2 s, …; after three the network is reported as unreachable.
    let mut seen = None;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let s = run.snapshot();
        let g = s.nets.iter().find(|n| n.id == "ghost").unwrap();
        if g.state == NetState::Unreachable {
            seen = Some((g.error.clone(), g.errors));
            break;
        }
    }
    let (err, errors) = seen.expect("the dead network should be flagged unreachable");
    assert!(err.is_some_and(|e| !e.is_empty()), "an unreachable network must say what went wrong");
    assert!(errors >= 3);
    let out = tokio::time::timeout(Duration::from_secs(90), h).await.expect("hung").unwrap().unwrap();
    assert_eq!(out, Outcome::Completed { size });
    assert_file_matches(&dir.path().join("out.bin.idmx"), size);
    assert_eq!(bytes_of(&run, "ghost"), 0);
}

#[tokio::test]
async fn lists_every_connection_and_what_it_is_doing() {
    let size = 30 << 20;
    let o = start(Config { size, per_conn_bps: 2_000_000, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let run = Run::new(cfg(o.url("f.bin"), size, dir.path(), two("a", "b"))).unwrap();
    let r2 = run.clone();
    let h = tokio::spawn(async move { r2.drive().await });
    tokio::time::sleep(Duration::from_millis(2000)).await;
    let streams = run.streams_snapshot();
    assert!(streams.len() >= 4, "{} connections", streams.len());
    let busy: Vec<_> = streams.iter().filter(|s| s.state == "downloading").collect();
    assert!(busy.len() >= 2);
    assert!(busy.iter().all(|s| s.block.is_some() && s.speed > 0.0 && s.bytes > 0), "{busy:?}");
    assert!(streams.iter().any(|s| s.net == "a") && streams.iter().any(|s| s.net == "b"));
    // Ids are unique.
    let mut ids: Vec<_> = streams.iter().map(|s| s.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), streams.len());
    run.stop();
    let _ = h.await;
}

#[tokio::test]
async fn the_map_credits_blocks_to_the_networks_that_delivered_them() {
    let size = 20 << 20;
    let o = start(Config { size, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let run = Run::new(cfg(o.url("f.bin"), size, dir.path(), two("a", "b"))).unwrap();
    run.drive().await.unwrap();
    let m = run.block_map();
    assert!(m.owner.iter().all(|o| *o < 2));
    // Any block shared between networks reports shares that add up.
    for (_, parts) in &m.split {
        let sum: u32 = parts.iter().map(|p| p.1 as u32).sum();
        assert!((98..=102).contains(&sum), "{parts:?}");
    }
}

// ───────────── single-connection mode: the user picks the network ─────────────

fn single_cfg(url: String, size: u64, dir: &std::path::Path, nets: Vec<NetPlan>) -> RunConfig {
    let mut c = cfg(url, size, dir, nets);
    c.ranges = false;
    c.validator = None;
    c
}

#[tokio::test]
async fn single_connection_downloads_use_one_network_and_move_to_the_next_when_it_is_switched_off() {
    let size = 24 << 20;
    let o = start(Config { size, ranges: false, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let run = Run::new(single_cfg(o.url("f.bin"), size, dir.path(), two_limited("a", "b", 3_000_000))).unwrap();
    let r2 = run.clone();
    let h = tokio::spawn(async move { r2.drive().await });
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(bytes_of(&run, "a") > 0, "the first network carries it");
    assert_eq!(bytes_of(&run, "b"), 0, "and only that one");
    assert_eq!(run.streams_snapshot().len(), 1, "one connection, shown in the table");

    // Switch it off: without ranges there's nothing to resume, so it starts again on the other.
    run.set_network_enabled("a", false);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(bytes_of(&run, "b") > 0, "the next network takes over");
    let out = tokio::time::timeout(Duration::from_secs(60), h).await.expect("hung").unwrap().unwrap();
    assert_eq!(out, Outcome::Completed { size });
    assert_file_matches(&dir.path().join("out.bin.idmx"), size);
}

#[tokio::test]
async fn single_connection_download_waits_for_a_network_to_be_switched_on() {
    let size = 6 << 20;
    let o = start(Config { size, ranges: false, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let mut nets = two("a", "b");
    nets[0].enabled = false;
    nets[1].enabled = false;
    let run = Run::new(single_cfg(o.url("f.bin"), size, dir.path(), nets)).unwrap();
    let r2 = run.clone();
    let h = tokio::spawn(async move { r2.drive().await });
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(!h.is_finished());
    assert_eq!(run.snapshot().downloaded, 0, "nothing may flow while every network is off");
    assert_eq!(o.requests.load(std::sync::atomic::Ordering::Relaxed), 0, "not even a request");
    run.set_network_enabled("b", true);
    let out = tokio::time::timeout(Duration::from_secs(30), h).await.expect("hung").unwrap().unwrap();
    assert_eq!(out, Outcome::Completed { size });
    assert_eq!(bytes_of(&run, "a"), 0);
    assert_file_matches(&dir.path().join("out.bin.idmx"), size);
}
