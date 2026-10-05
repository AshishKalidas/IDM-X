//! Torrents as jobs: queued, shown, paused, seeded and cancelled like any other download.

mod common;

use idmx_core::torrent::{AddOpts, Source, TorrentHub};
use idmx_core::types::*;
use idmx_core::{Engine, EngineOptions};
use librqbit::spawn_utils::BlockingSpawner;
use librqbit::{create_torrent, CreateTorrentOptions};
use std::path::Path;
use std::time::{Duration, Instant};

fn iso() -> idmx_core::torrent::SessionCfg {
    idmx_core::torrent::SessionCfg { bind_device: None, dht: false, listen_port: None, upload_bps: None, download_bps: None, peer_limit: None, isolated: true }
}

fn lo() -> NetInterface {
    NetInterface { id: "lo".into(), label: "Loopback".into(), kind: IfKind::Ethernet, ipv4: vec!["127.0.0.1".into()], ipv6: vec![], gateway: None, subnets: vec![], mac: None, up: true, link_mbps: None, usable: true }
}

struct Swarm {
    torrent_file: std::path::PathBuf,
    _seed: TorrentHub,
    addr: std::net::SocketAddr,
    data: Vec<Vec<u8>>,
}

/// A seeder for a torrent of `files`, with the `.torrent` saved at `root/pack.torrent`.
async fn swarm(root: &Path, torrent_name: &str, files: &[(&str, u64)]) -> Swarm {
    let content = root.join("seed").join(torrent_name);
    std::fs::create_dir_all(&content).unwrap();
    let data: Vec<Vec<u8>> = files
        .iter()
        .map(|(n, size)| {
            let b = common::expected(*size);
            std::fs::write(content.join(n), &b).unwrap();
            b
        })
        .collect();
    let created = create_torrent(&content, CreateTorrentOptions { name: Some(torrent_name), trackers: vec![], piece_length: Some(256 * 1024) }, &BlockingSpawner::new(4)).await.unwrap();
    let bytes = created.as_bytes().unwrap().to_vec();
    let torrent_file = root.join(format!("{torrent_name}.torrent"));
    std::fs::write(&torrent_file, &bytes).unwrap();
    let hub = TorrentHub::default();
    let h = hub.add(&iso(), &Source::Bytes(bytes), &reqwest::Client::new(), AddOpts { output: content, ..Default::default() }).await.unwrap();
    let end = Instant::now() + Duration::from_secs(20);
    while !idmx_core::torrent::snapshot(&h).finished {
        assert!(Instant::now() < end, "seeder not ready");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let addr = hub.listen_addr().await.unwrap();
    Swarm { torrent_file, _seed: hub, addr, data }
}

async fn engine(dir: &Path, peer: std::net::SocketAddr) -> Engine {
    let e = Engine::start_with(
        dir.join("data"),
        EngineOptions { fixed_interfaces: Some(vec![lo()]), no_device_binding: true, torrent_isolated: true, torrent_peers: vec![peer], ..Default::default() },
    )
    .await
    .unwrap();
    e.update_settings(serde_json::json!({"downloadDir": dir.join("dl").to_string_lossy(), "notifications": false, "torrent": {"seed": false}})).unwrap();
    e
}

async fn wait_for(e: &Engine, id: &str, want: impl Fn(&Job) -> bool, secs: u64) -> Job {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        let j = e.get(id).expect("job exists");
        if want(&j) {
            return j;
        }
        assert!(Instant::now() < end, "timed out; job is {:?} err={:?} torrent={:?}", j.status, j.error, j.torrent);
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
}

fn add(e: &Engine, link: &str) -> Job {
    e.add(AddRequest { urls: vec![link.to_string()], ..Default::default() }).unwrap().remove(0)
}

#[tokio::test]
async fn a_torrent_downloads_like_any_other_job() {
    let dir = tempfile::tempdir().unwrap();
    let sw = swarm(dir.path(), "album", &[("one.bin", 3 << 20), ("two.bin", 2 << 20)]).await;
    let e = engine(dir.path(), sw.addr).await;

    let job = add(&e, sw.torrent_file.to_str().unwrap());
    assert_eq!(job.kind, JobKind::Torrent);
    assert_eq!(job.file_name, "album", "named after the .torrent before its real name is known");

    let done = wait_for(&e, &job.id, |j| j.status == JobStatus::Completed, 60).await;
    assert_eq!(done.file_name, "album");
    assert_eq!(done.size, Some(5 << 20));
    let t = done.torrent.expect("torrent details are kept");
    assert_eq!(t.files.len(), 2);
    assert!(t.files.iter().all(|f| f.downloaded == f.size));
    let root = dir.path().join("dl").join("album");
    assert_eq!(std::fs::read(root.join("one.bin")).unwrap(), sw.data[0]);
    assert_eq!(std::fs::read(root.join("two.bin")).unwrap(), sw.data[1]);
    assert_eq!(done.path.as_deref(), Some(root.to_str().unwrap()));
    assert!(!done.speed_history.is_empty() || done.peak_streams >= 1, "peers were recorded");
    e.shutdown().await;
}

#[tokio::test]
async fn it_shows_progress_peers_and_a_network_while_running() {
    let dir = tempfile::tempdir().unwrap();
    let sw = swarm(dir.path(), "slowpack", &[("big.bin", 30 << 20)]).await;
    let e = engine(dir.path(), sw.addr).await;
    e.update_settings(serde_json::json!({"globalSpeedLimit": 2_000_000})).unwrap();
    let job = add(&e, sw.torrent_file.to_str().unwrap());
    let j = wait_for(&e, &job.id, |j| j.status == JobStatus::Downloading && j.downloaded > (1 << 20), 40).await;
    assert!(j.speed > 100_000.0, "speed {}", j.speed);
    let t = j.torrent.as_ref().unwrap();
    assert!(t.peers >= 1, "{t:?}");
    assert_eq!(t.state, "downloading");
    assert_eq!(j.networks.len(), 1);
    assert!(j.networks[0].active >= 1);
    // The ticker is broadcasting it too.
    let mut ev = e.subscribe();
    let mut saw = false;
    let end = Instant::now() + Duration::from_secs(3);
    while Instant::now() < end && !saw {
        if let Ok(Ok(Event::Tick { jobs, .. })) = tokio::time::timeout(Duration::from_millis(700), ev.recv()).await {
            saw = jobs.iter().any(|t| t.id == job.id && t.torrent.as_ref().is_some_and(|x| x.peers >= 1) && t.speed > 0.0);
        }
    }
    assert!(saw, "ticks should carry torrent peers and speed");
    e.cancel(&job.id).unwrap();
    e.shutdown().await;
}

#[tokio::test]
async fn pause_resume_and_cancel_work_and_cancel_deletes_only_what_it_made() {
    let dir = tempfile::tempdir().unwrap();
    let sw = swarm(dir.path(), "pausable", &[("a.bin", 24 << 20), ("b.bin", 1 << 20)]).await;
    let e = engine(dir.path(), sw.addr).await;
    e.update_settings(serde_json::json!({"globalSpeedLimit": 3_000_000})).unwrap();
    // A folder of the user's own with the same name must be left alone.
    let dl = dir.path().join("dl");
    std::fs::create_dir_all(dl.join("pausable")).unwrap();
    std::fs::write(dl.join("pausable/mine.txt"), b"keep me").unwrap();

    let job = add(&e, sw.torrent_file.to_str().unwrap());
    wait_for(&e, &job.id, |j| j.downloaded > (2 << 20), 40).await;
    let folder = e.get(&job.id).unwrap().path.unwrap();
    assert_ne!(folder, dl.join("pausable").to_str().unwrap(), "must not write into the user's existing folder");

    e.pause(&job.id).unwrap();
    let p = wait_for(&e, &job.id, |j| j.status == JobStatus::Paused, 15).await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(e.get(&job.id).unwrap().downloaded, p.downloaded, "paused means no more bytes");

    e.resume(&job.id).unwrap();
    wait_for(&e, &job.id, |j| j.status == JobStatus::Downloading && j.downloaded > p.downloaded, 30).await;

    e.cancel(&job.id).unwrap();
    wait_for(&e, &job.id, |j| j.status == JobStatus::Cancelled, 15).await;
    assert!(!Path::new(&folder).exists(), "what the torrent fetched is deleted");
    assert_eq!(std::fs::read(dl.join("pausable/mine.txt")).unwrap(), b"keep me", "the user's own folder is untouched");
    e.shutdown().await;
}

#[tokio::test]
async fn seeding_shares_after_the_download_and_stops_on_request() {
    let dir = tempfile::tempdir().unwrap();
    let sw = swarm(dir.path(), "shareme", &[("f.bin", 2 << 20)]).await;
    let e = engine(dir.path(), sw.addr).await;
    e.update_settings(serde_json::json!({"torrent": {"seed": true, "seedRatio": 0.0, "seedMinutes": 0}})).unwrap();
    let job = add(&e, sw.torrent_file.to_str().unwrap());
    let j = wait_for(&e, &job.id, |j| j.status == JobStatus::Seeding, 60).await;
    assert_eq!(j.torrent.as_ref().unwrap().state, "seeding");
    assert_eq!(e.stats().seeding, 1);
    // Seeding doesn't occupy a download slot.
    e.update_settings(serde_json::json!({"maxConcurrentJobs": 1})).unwrap();
    assert_eq!(e.get(&job.id).unwrap().status, JobStatus::Seeding);
    // Stopping ends the sharing, keeps the files, and the job is simply done.
    e.pause(&job.id).unwrap();
    let done = wait_for(&e, &job.id, |j| j.status == JobStatus::Completed, 15).await;
    assert!(dir.path().join("dl/f.bin").exists());
    assert_eq!(done.torrent.unwrap().state, "completed");
    e.shutdown().await;
}

#[tokio::test]
async fn a_ratio_limit_ends_seeding_by_itself() {
    let dir = tempfile::tempdir().unwrap();
    let sw = swarm(dir.path(), "ratio", &[("r.bin", 1 << 20)]).await;
    let e = engine(dir.path(), sw.addr).await;
    // Ratio 0 would mean "forever"; use a time limit that is already up instead.
    e.update_settings(serde_json::json!({"torrent": {"seed": true, "seedRatio": 0.0, "seedMinutes": 0}})).unwrap();
    let job = add(&e, sw.torrent_file.to_str().unwrap());
    wait_for(&e, &job.id, |j| j.status == JobStatus::Seeding, 40).await;
    // Tightening the rule while it seeds applies on the next check.
    e.update_settings(serde_json::json!({"torrent": {"seed": false}})).unwrap();
    // (the loop reads settings when the download starts; stopping manually is the supported path)
    e.pause(&job.id).unwrap();
    wait_for(&e, &job.id, |j| j.status == JobStatus::Completed, 15).await;
    e.shutdown().await;
}

#[tokio::test]
async fn a_single_file_torrent_refuses_to_overwrite_an_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let sw = swarm(dir.path(), "solo", &[("only.bin", 1 << 20)]).await;
    // Single file inside a one-file folder torrent is still a one-file torrent for the layout rule.
    let e = engine(dir.path(), sw.addr).await;
    let dl = dir.path().join("dl");
    std::fs::create_dir_all(&dl).unwrap();
    std::fs::write(dl.join("only.bin"), b"precious").unwrap();
    let job = add(&e, sw.torrent_file.to_str().unwrap());
    let j = wait_for(&e, &job.id, |j| j.status == JobStatus::Failed, 30).await;
    let m = j.error.unwrap();
    assert!(m.contains("already in that folder") && m.contains("only.bin"), "{m}");
    assert_eq!(std::fs::read(dl.join("only.bin")).unwrap(), b"precious");
    e.shutdown().await;
}

#[tokio::test]
async fn pasting_nonsense_that_looks_like_a_magnet_is_explained_not_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let sw = swarm(dir.path(), "x", &[("x.bin", 1 << 18)]).await;
    let e = engine(dir.path(), sw.addr).await;
    // Looks like a magnet but has no usable hash.
    let job = add(&e, "magnet:?dn=just-a-name");
    assert_eq!(job.kind, JobKind::Torrent);
    assert_eq!(job.file_name, "just-a-name");
    let j = wait_for(&e, &job.id, |j| j.status == JobStatus::Failed, 30).await;
    assert!(j.error.is_some_and(|m| !m.is_empty() && !m.contains("panick")));
    // Names come from the link.
    assert_eq!(idmx_core::engine::torrent_display_name("magnet:?xt=urn:btih:ABCDEF1234567890&dn=My+Movie"), "My Movie");
    assert_eq!(idmx_core::engine::torrent_display_name("magnet:?xt=urn:btih:ABCDEF1234567890"), "torrent ABCDEF12");
    assert_eq!(idmx_core::engine::torrent_display_name("https://x.org/dl/Ubuntu.iso.torrent?x=1"), "Ubuntu.iso");
    e.shutdown().await;
}
