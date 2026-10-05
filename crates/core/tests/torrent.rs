//! A real BitTorrent transfer over loopback: one session seeds, IDMX's wrapper downloads.

mod common;

use idmx_core::torrent::*;
use librqbit::spawn_utils::BlockingSpawner;
use librqbit::{create_torrent, CreateTorrentOptions};
use std::time::{Duration, Instant};

fn cfg() -> SessionCfg {
    SessionCfg { bind_device: None, dht: false, listen_port: None, upload_bps: None, download_bps: None, peer_limit: None, isolated: true }
}

/// Make a torrent of `files` (name, size) and start a seeder for it. Returns the .torrent bytes,
/// the seeder's listen address, and what is needed to keep it alive.
async fn seeder(root: &std::path::Path, files: &[(&str, u64)]) -> (Vec<u8>, std::net::SocketAddr, TorrentHub, Vec<Vec<u8>>) {
    let content = root.join("seed").join("pack");
    std::fs::create_dir_all(&content).unwrap();
    let mut data = Vec::new();
    for (name, size) in files {
        let bytes = common::expected(*size);
        std::fs::write(content.join(name), &bytes).unwrap();
        data.push(bytes);
    }
    let created = create_torrent(&content, CreateTorrentOptions { name: Some("pack"), trackers: vec![], piece_length: Some(256 * 1024) }, &BlockingSpawner::new(4)).await.unwrap();
    let torrent = created.as_bytes().unwrap().to_vec();
    let hub = TorrentHub::default();
    let h = hub
        .add(&cfg(), &Source::Bytes(torrent.clone()), &reqwest::Client::new(), AddOpts { output: content.clone(), ..Default::default() })
        .await
        .unwrap();
    // Wait until the seeder has checked its files and is ready to serve.
    let end = Instant::now() + Duration::from_secs(20);
    while !snapshot(&h).finished {
        assert!(Instant::now() < end, "seeder never became ready: {:?}", snapshot(&h));
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let addr = hub.listen_addr().await.expect("seeder listens");
    (torrent, addr, hub, data)
}

async fn wait_done(h: &Handle, secs: u64) -> Snapshot {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        let s = snapshot(h);
        if s.finished {
            return s;
        }
        assert!(Instant::now() < end, "timed out: {s:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn downloads_a_multi_file_torrent_from_a_peer_and_the_bytes_match() {
    let dir = tempfile::tempdir().unwrap();
    let (torrent, peer, _seed, data) = seeder(dir.path(), &[("a.bin", 3 << 20), ("b.bin", 1_500_001)]).await;

    let hub = TorrentHub::default();
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let h = hub
        .add(&cfg(), &Source::Bytes(torrent), &reqwest::Client::new(), AddOpts { output: out.join("pack"), initial_peers: vec![peer], ..Default::default() })
        .await
        .unwrap();
    let s = wait_done(&h, 60).await;
    assert_eq!(s.downloaded, s.total);
    assert_eq!(s.total, (3 << 20) + 1_500_001);
    assert_eq!(s.name, "pack");
    assert_eq!(s.files.len(), 2);
    assert!(s.files.iter().all(|f| f.1 == f.2 && f.3), "every file complete and selected: {:?}", s.files);
    assert_eq!(std::fs::read(out.join("pack/a.bin")).unwrap(), data[0]);
    assert_eq!(std::fs::read(out.join("pack/b.bin")).unwrap(), data[1]);
}

#[tokio::test]
async fn can_pick_files_and_inspect_before_downloading() {
    let dir = tempfile::tempdir().unwrap();
    let (torrent, peer, _seed, data) = seeder(dir.path(), &[("keep.bin", 2 << 20), ("skip.bin", 2 << 20)]).await;

    let hub = TorrentHub::default();
    let meta = hub.inspect(&cfg(), &Source::Bytes(torrent.clone()), &reqwest::Client::new(), &[peer], Duration::from_secs(20)).await.unwrap();
    assert_eq!(meta.name, "pack");
    assert_eq!(meta.total, 4 << 20);
    assert_eq!(meta.files.len(), 2);
    let keep = meta.files.iter().find(|f| f.path.ends_with("keep.bin")).unwrap().index;

    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let h = hub
        .add(&cfg(), &Source::Bytes(torrent), &reqwest::Client::new(), AddOpts { output: out.join("pack"), only_files: Some(vec![keep as usize]), initial_peers: vec![peer], ..Default::default() })
        .await
        .unwrap();
    let s = wait_done(&h, 60).await;
    assert_eq!(s.total, 2 << 20, "progress is measured against what was asked for");
    assert_eq!(std::fs::read(out.join("pack/keep.bin")).unwrap(), data[0]);
    assert!(!out.join("pack/skip.bin").exists() || std::fs::metadata(out.join("pack/skip.bin")).unwrap().len() == 0 || std::fs::read(out.join("pack/skip.bin")).unwrap() != data[1], "the skipped file must not be downloaded");
    assert!(s.files.iter().any(|f| !f.3), "one file is marked unselected");
}

#[tokio::test]
async fn pause_and_resume_a_torrent() {
    let dir = tempfile::tempdir().unwrap();
    let (torrent, peer, _seed, data) = seeder(dir.path(), &[("slow.bin", 24 << 20)]).await;

    // Throttle the downloader so there is time to pause it mid-way.
    let hub = TorrentHub::default();
    let slow = SessionCfg { download_bps: Some(2_000_000), ..cfg() };
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let h = hub
        .add(&slow, &Source::Bytes(torrent), &reqwest::Client::new(), AddOpts { output: out.join("pack"), initial_peers: vec![peer], ..Default::default() })
        .await
        .unwrap();
    let end = Instant::now() + Duration::from_secs(30);
    while snapshot(&h).downloaded < (2 << 20) {
        assert!(Instant::now() < end, "never started: {:?}", snapshot(&h));
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    hub.pause(&h).await.unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    let a = snapshot(&h);
    assert_eq!(a.state, "paused");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(snapshot(&h).downloaded, a.downloaded, "paused means no more bytes");
    assert!(a.downloaded < a.total);

    hub.unpause(&h).await.unwrap();
    let s = wait_done(&h, 90).await;
    assert_eq!(s.downloaded, s.total);
    assert_eq!(std::fs::read(out.join("pack/slow.bin")).unwrap(), data[0]);
}

#[tokio::test]
async fn bad_input_is_explained() {
    let hub = TorrentHub::default();
    let http = reqwest::Client::new();
    let e = hub.inspect(&cfg(), &Source::Bytes(b"this is not a torrent".to_vec()), &http, &[], Duration::from_secs(5)).await.unwrap_err();
    let m = e.to_string();
    assert!(!m.is_empty() && !m.contains("panicked"), "{m}");
    let e = hub.inspect(&cfg(), &Source::File("/no/such/file.torrent".into()), &http, &[], Duration::from_secs(5)).await.unwrap_err();
    assert!(e.to_string().contains("Couldn't read"), "{e}");
    let e = hub.inspect(&cfg(), &Source::Url("http://127.0.0.1:9/x.torrent".into()), &http, &[], Duration::from_secs(5)).await.unwrap_err();
    assert!(e.to_string().contains("refused the connection"), "{e}");
}

#[tokio::test]
async fn files_are_split_across_two_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let (torrent, peer, _seed, data) = seeder(dir.path(), &[("a.bin", 3 << 20), ("b.bin", 3 << 20), ("c.bin", 1 << 20), ("d.bin", 1 << 20)]).await;

    let hub = TorrentHub::default();
    let lo = if cfg!(target_os = "macos") { "lo0" } else { "lo" };
    let cfgs = [cfg(), SessionCfg { bind_device: Some(lo.into()), ..cfg() }];
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let files: Vec<(usize, u64)> = vec![(0, 3 << 20), (1, 3 << 20), (2, 1 << 20), (3, 1 << 20)];
    let h = hub.add_multi(&cfgs, &Source::Bytes(torrent), &reqwest::Client::new(), AddOpts { output: out.join("pack"), initial_peers: vec![peer], ..Default::default() }, &files).await.unwrap();
    assert_eq!(h.parts.len(), 2, "one session per network");
    let s = wait_done(&h, 60).await;
    assert_eq!(s.total, 8 << 20);
    assert_eq!(s.downloaded, s.total);
    assert_eq!(s.parts.len(), 2);
    assert!(s.parts.iter().all(|p| p.downloaded >= 4 << 20), "each network carried its share: {:?}", s.parts);
    for (i, n) in ["a.bin", "b.bin", "c.bin", "d.bin"].iter().enumerate() {
        assert_eq!(std::fs::read(out.join("pack").join(n)).unwrap(), data[i], "{n}");
    }
    assert!(s.files.iter().all(|f| f.1 == f.2 && f.3), "{:?}", s.files);
    hub.shutdown().await;
}
