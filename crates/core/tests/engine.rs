mod common;

use common::*;
use idmx_core::types::*;
use idmx_core::{Engine, EngineOptions};
use std::path::Path;
use std::time::{Duration, Instant};

fn lo() -> NetInterface {
    NetInterface {
        id: "lo-test".into(),
        label: "Loopback test".into(),
        kind: IfKind::Ethernet,
        ipv4: vec!["127.0.0.1".into()],
        ipv6: vec![],
        gateway: None,
        subnets: vec![],
        mac: None,
        up: true,
        link_mbps: None,
        usable: true,
    }
}

async fn engine(dir: &Path) -> Engine {
    let e = Engine::start_with(dir.join("data"), EngineOptions { fixed_interfaces: Some(vec![lo()]), no_device_binding: true, ..Default::default() }).await.unwrap();
    e.update_settings(serde_json::json!({
        "downloadDir": dir.join("dl").to_string_lossy(),
        "notifications": false,
    }))
    .unwrap();
    e
}

async fn wait_for(e: &Engine, id: &str, want: impl Fn(&Job) -> bool, secs: u64) -> Job {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        let j = e.get(id).expect("job exists");
        if want(&j) {
            return j;
        }
        assert!(Instant::now() < end, "timed out; job is {:?} ({:?}) {:?}", j.status, j.error, j.log);
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

fn add(e: &Engine, url: String) -> Job {
    e.add(AddRequest { urls: vec![url], ..Default::default() }).unwrap().remove(0)
}

#[tokio::test]
async fn full_lifecycle_with_header_checksum_and_correct_name() {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    let size = 6 << 20;
    let sha = base64::engine::general_purpose::STANDARD.encode(Sha256::digest(expected(size)));
    let o = start(Config { size, digest_sha256_b64: Some(sha), disposition: Some("attachment; filename=\"real name.bin\"".into()), ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path()).await;

    let job = add(&e, o.url("whatever"));
    let done = wait_for(&e, &job.id, |j| j.status == JobStatus::Completed, 30).await;

    assert_eq!(done.file_name, "real name.bin");
    let path = done.path.clone().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), expected(size));
    assert!(!Path::new(&format!("{path}.idmx")).exists(), "staging file should be gone");
    let c = done.checksum.unwrap();
    assert_eq!((c.verified, c.source.as_deref()), (Some(true), Some("header")));
    assert_eq!(done.networks.len(), 1);
    assert!(done.networks[0].bytes >= size);
    e.shutdown().await;
}

#[tokio::test]
async fn same_name_twice_never_overwrites() {
    let o = start(Config { size: 1 << 20, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path()).await;
    let a = add(&e, o.url("a/same.bin"));
    let b = add(&e, o.url("b/same.bin"));
    let a = wait_for(&e, &a.id, |j| j.status == JobStatus::Completed, 30).await;
    let b = wait_for(&e, &b.id, |j| j.status == JobStatus::Completed, 30).await;
    let mut names = vec![a.file_name, b.file_name];
    names.sort();
    assert_eq!(names, ["same (1).bin", "same.bin"]);
    e.shutdown().await;
}

#[tokio::test]
async fn wrong_user_checksum_fails_and_leaves_nothing_behind() {
    let o = start(Config { size: 2 << 20, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path()).await;
    let job = e
        .add(AddRequest {
            urls: vec![o.url("c.bin")],
            options: JobOptions { checksum: Some(format!("sha256:{}", "0".repeat(64))), ..Default::default() },
            ..Default::default()
        })
        .unwrap()
        .remove(0);
    let j = wait_for(&e, &job.id, |j| j.status == JobStatus::Failed, 30).await;
    let m = j.error.unwrap();
    assert!(m.contains("arrived damaged") && m.contains("deleted"), "{m}");
    let dl = dir.path().join("dl");
    let leftovers: Vec<_> = std::fs::read_dir(&dl).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
    e.shutdown().await;
}

#[tokio::test]
async fn queue_respects_the_concurrency_limit_and_priority() {
    let o = start(Config { size: 3 << 20, per_conn_bps: 3_000_000, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path()).await;
    e.update_settings(serde_json::json!({"maxConcurrentJobs": 1, "defaultStreams": 1, "maxStreams": 2})).unwrap();

    let jobs = e.add(AddRequest { urls: vec![o.url("q[1-3].bin")], paused: true, ..Default::default() }).unwrap();
    assert_eq!(jobs.len(), 3, "pattern should expand to three jobs");
    // Highest priority first, regardless of the order they were added.
    e.set_priority(&jobs[2].id, 10).unwrap();
    for j in &jobs {
        e.resume(&j.id).unwrap();
    }
    let end = Instant::now() + Duration::from_secs(40);
    let mut order = Vec::new();
    let mut max_running = 0;
    while Instant::now() < end {
        let list = e.list();
        let running = list.iter().filter(|j| matches!(j.status, JobStatus::Downloading | JobStatus::Probing)).count();
        max_running = max_running.max(running);
        for j in &list {
            if j.status == JobStatus::Completed && !order.contains(&j.id) {
                order.push(j.id.clone());
            }
        }
        if order.len() == 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert_eq!(order.len(), 3, "not all finished: {:?}", e.list().iter().map(|j| j.status).collect::<Vec<_>>());
    assert_eq!(max_running, 1, "more than one ran at once");
    assert_eq!(order[0], jobs[2].id, "priority job should go first");
    e.shutdown().await;
}

#[tokio::test]
async fn pause_resume_and_survive_a_restart() {
    let size = 20 << 20;
    let o = start(Config { size, per_conn_bps: 2_000_000, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path()).await;
    e.update_settings(serde_json::json!({"defaultStreams": 2, "maxStreams": 2})).unwrap();
    let job = add(&e, o.url("big.bin"));
    wait_for(&e, &job.id, |j| j.downloaded > (1 << 20), 30).await;

    e.pause(&job.id).unwrap();
    let paused = wait_for(&e, &job.id, |j| j.status == JobStatus::Paused, 10).await;
    assert!(paused.downloaded > 0 && paused.downloaded < size);
    let bytes_before = paused.downloaded;

    // Quit the app entirely, then reopen it on the same data directory.
    e.shutdown().await;
    drop(e);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let e2 = engine(dir.path()).await;
    let back = e2.get(&job.id).expect("job survives a restart");
    assert_eq!(back.status, JobStatus::Paused);
    assert_eq!(back.url, job.url);
    assert!(e2.block_map(&job.id).is_some(), "progress map should be restored");

    e2.update_settings(serde_json::json!({"defaultStreams": 8, "maxStreams": 16})).unwrap();
    e2.resume(&job.id).unwrap();
    let done = wait_for(&e2, &job.id, |j| j.status == JobStatus::Completed, 60).await;
    assert_eq!(std::fs::read(done.path.unwrap()).unwrap(), expected(size));
    // It picked up from where it stopped rather than from zero.
    let moved: u64 = done.networks.iter().map(|n| n.bytes).sum();
    assert!(moved < size - bytes_before / 2, "second leg fetched {moved} of {size} (had {bytes_before})");
    e2.shutdown().await;
}

#[tokio::test]
async fn post_rules_move_and_run() {
    let o = start(Config { size: 1 << 20, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path()).await;
    let sorted = dir.path().join("sorted");
    e.update_settings(serde_json::json!({
        "postRules": [{
            "id": "1", "name": "bins", "enabled": true,
            "when": { "extensions": ["bin"] },
            "actions": [ { "type": "moveTo", "dir": sorted.to_string_lossy() } ]
        }]
    }))
    .unwrap();
    let job = add(&e, o.url("thing.bin"));
    let done = wait_for(&e, &job.id, |j| j.status == JobStatus::Completed, 30).await;
    assert_eq!(done.path.as_deref(), Some(sorted.join("thing.bin").to_str().unwrap()));
    assert!(sorted.join("thing.bin").exists());
    assert!(done.log.iter().any(|l| l.contains("moved to")), "{:?}", done.log);
    e.shutdown().await;
}

#[tokio::test]
async fn cancel_removes_partial_data_and_remove_forgets_the_job() {
    let o = start(Config { size: 30 << 20, per_conn_bps: 1_000_000, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path()).await;
    let job = add(&e, o.url("x.bin"));
    wait_for(&e, &job.id, |j| j.downloaded > 0, 20).await;
    e.cancel(&job.id).unwrap();
    wait_for(&e, &job.id, |j| j.status == JobStatus::Cancelled, 10).await;
    assert!(std::fs::read_dir(dir.path().join("dl")).unwrap().next().is_none(), "partial file should be deleted");
    e.remove(&job.id, false).unwrap();
    assert!(e.get(&job.id).is_none());
    e.shutdown().await;
}

#[tokio::test]
async fn rejects_garbage_input_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path()).await;
    assert!(e.add(AddRequest { urls: vec!["not a url".into()], ..Default::default() }).is_err());
    assert!(e.add(AddRequest { urls: vec!["ftp://example.com/x".into()], ..Default::default() }).is_err());
    assert!(e.add(AddRequest { urls: vec![], ..Default::default() }).is_err());
    assert!(e.pause("nope").is_err());
    e.shutdown().await;
}

#[tokio::test]
async fn a_dead_server_fails_the_job_instead_of_hanging() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path()).await;
    // Nothing listens on this port.
    let job = add(&e, "http://127.0.0.1:9/nothing.bin".into());
    let j = wait_for(&e, &job.id, |j| j.status == JobStatus::Failed, 40).await;
    let msg = j.error.unwrap();
    assert!(msg.contains("refused the connection") && !msg.contains("error sending request"), "{msg}");
    e.shutdown().await;
}

#[tokio::test]
async fn second_instance_on_the_same_data_dir_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path()).await;
    let second = Engine::start_with(dir.path().join("data"), EngineOptions::default()).await;
    assert!(second.is_err());
    e.shutdown().await;
}

// ───────────── live network switches, ping, updates, subnets, changed files ─────────────

fn lo_named(id: &str, subnet: &str) -> NetInterface {
    NetInterface { id: id.into(), label: id.into(), subnets: vec![subnet.into()], ..lo() }
}

async fn engine_with(dir: &Path, nets: Vec<NetInterface>, tweak: impl FnOnce(&mut EngineOptions)) -> Engine {
    let mut o = EngineOptions { fixed_interfaces: Some(nets), no_device_binding: true, ..Default::default() };
    tweak(&mut o);
    let e = Engine::start_with(dir.join("data"), o).await.unwrap();
    e.update_settings(serde_json::json!({"downloadDir": dir.join("dl").to_string_lossy(), "notifications": false})).unwrap();
    e
}

#[tokio::test]
async fn networks_can_be_switched_per_download_while_it_runs() {
    let size = 80 << 20;
    let o = start(Config { size, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let e = engine_with(dir.path(), vec![lo_named("a", "10.1.0.0/24"), lo_named("b", "10.2.0.0/24")], |_| {}).await;
    // 2 MB/s per network, so there is time to flip switches.
    e.update_settings(serde_json::json!({"interfaces": {
        "a": {"metered": false, "capPeriod": "monthly", "speedLimit": 2_000_000},
        "b": {"metered": false, "capPeriod": "monthly", "speedLimit": 2_000_000}
    }}))
    .unwrap();
    let job = add(&e, o.url("big.bin"));
    wait_for(&e, &job.id, |j| j.networks.len() == 2 && j.networks.iter().all(|n| n.bytes > 0), 20).await;

    e.set_job_network(&job.id, "b", false).unwrap();
    let j = wait_for(&e, &job.id, |j| j.networks.iter().any(|n| n.id == "b" && !n.enabled), 5).await;
    assert_eq!(j.options.interfaces.as_deref(), Some(&["a".to_string()][..]), "the choice is remembered on the download");
    let b_then = e.get(&job.id).unwrap().networks.iter().find(|n| n.id == "b").unwrap().bytes;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let now = e.get(&job.id).unwrap();
    assert_eq!(now.networks.iter().find(|n| n.id == "b").unwrap().state, NetState::Disabled);
    assert!(now.networks.iter().find(|n| n.id == "b").unwrap().bytes <= b_then + (1 << 20), "b kept transferring after being switched off");

    // Switching off the last one pauses the download rather than leaving it stuck.
    e.set_job_network(&job.id, "a", false).unwrap();
    wait_for(&e, &job.id, |j| j.status == JobStatus::Paused, 10).await;
    assert!(e.set_job_network("nope", "a", true).is_err());

    // Turn one back on and resume: it carries on with just that one.
    e.set_job_network(&job.id, "b", true).unwrap();
    e.resume(&job.id).unwrap();
    wait_for(&e, &job.id, |j| j.status == JobStatus::Downloading && j.networks.iter().any(|n| n.id == "b" && n.enabled), 20).await;
    let streams = e.streams(&job.id).unwrap();
    assert!(!streams.is_empty() && streams.iter().any(|s| s.state == "downloading"), "{streams:?}");
    e.cancel(&job.id).unwrap();
    e.shutdown().await;
}

#[tokio::test]
async fn pings_each_network_and_notices_when_nothing_gets_through() {
    let o = start(Config::default()).await;
    let dir = tempfile::tempdir().unwrap();
    let e = engine_with(dir.path(), vec![lo_named("a", "10.1.0.0/24")], |op| op.ping_target = Some(o.addr)).await;
    let infos = e.ping_now().await;
    assert_eq!(infos[0].reachable, Some(true));
    assert!(infos[0].latency_ms.is_some_and(|ms| ms < 500.0));
    assert_eq!(infos[0].state, NetState::Idle);
    e.shutdown().await;
    drop(e);
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Point it at a port nobody listens on: the network is "connected" but gets nowhere.
    let dir2 = tempfile::tempdir().unwrap();
    let e2 = engine_with(dir2.path(), vec![lo_named("a", "10.1.0.0/24")], |op| op.ping_target = Some("127.0.0.1:9".parse().unwrap())).await;
    let infos = e2.ping_now().await;
    assert_eq!(infos[0].reachable, Some(false));
    assert_eq!(infos[0].state, NetState::Unreachable);
    assert!(infos[0].latency_ms.is_none());
    e2.shutdown().await;
}

#[tokio::test]
async fn warns_about_networks_that_share_a_subnet() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine_with(
        dir.path(),
        vec![lo_named("wifi", "192.168.1.0/24"), lo_named("eth", "192.168.1.0/24"), lo_named("phone", "172.20.10.0/28")],
        |_| {},
    )
    .await;
    let infos = e.interfaces();
    let by = |id: &str| infos.iter().find(|i| i.iface.id == id).unwrap();
    assert_eq!(by("wifi").shares_subnet_with, vec!["eth".to_string()]);
    assert_eq!(by("eth").shares_subnet_with, vec!["wifi".to_string()]);
    assert!(by("phone").shares_subnet_with.is_empty(), "a different subnet is a genuinely separate link");
    e.shutdown().await;
}

#[tokio::test]
async fn finds_new_releases_ignores_old_ones_and_remembers_dismissal() {
    let dir = tempfile::tempdir().unwrap();
    let newer = release_feed(200, r#"{"tag_name":"v99.0.0","html_url":"https://example.com/releases/99","prerelease":false}"#).await;
    let url = format!("http://{newer}/latest");
    let e = engine_with(dir.path(), vec![lo()], |o| o.update_url = Some(url.clone())).await;
    let mut ev = e.subscribe();
    let found = e.check_update().await.unwrap().expect("a newer release exists");
    assert_eq!((found.version.as_str(), found.dismissed), ("99.0.0", false));
    assert_eq!(found.url, "https://example.com/releases/99");
    // …and it was announced.
    let mut announced = false;
    while let Ok(x) = ev.try_recv() {
        announced |= matches!(x, Event::Update(Some(_)));
    }
    assert!(announced);

    e.dismiss_update();
    assert!(e.update_info().unwrap().dismissed);
    e.shutdown().await;
    drop(e);
    tokio::time::sleep(Duration::from_millis(300)).await;

    // The dismissal survives a restart, for that version only.
    let e2 = engine_with(dir.path(), vec![lo()], |o| o.update_url = Some(url)).await;
    assert!(e2.check_update().await.unwrap().unwrap().dismissed);
    e2.shutdown().await;

    // Older, pre-release, draft, missing and broken feeds all mean "nothing to show".
    for (status, body) in [
        (200, r#"{"tag_name":"v0.0.1","html_url":"x"}"#),
        (200, r#"{"tag_name":"v100.0.0","html_url":"x","prerelease":true}"#),
        (200, r#"{"tag_name":"v100.0.0","html_url":"x","draft":true}"#),
        (404, "{}"),
    ] {
        let d = tempfile::tempdir().unwrap();
        let a = release_feed(status, body).await;
        let e = engine_with(d.path(), vec![lo()], |o| o.update_url = Some(format!("http://{a}/latest"))).await;
        assert!(e.check_update().await.unwrap().is_none(), "{body}");
        e.shutdown().await;
    }
    let d = tempfile::tempdir().unwrap();
    let broken = release_feed(200, "<html>not json</html>").await;
    let e = engine_with(d.path(), vec![lo()], |o| o.update_url = Some(format!("http://{broken}/latest"))).await;
    assert!(e.check_update().await.is_err());
    // With no source configured it simply does nothing.
    let e_none = engine_with(tempfile::tempdir().unwrap().path(), vec![lo()], |_| {}).await;
    assert!(e_none.check_update().await.unwrap().is_none());
}

#[tokio::test]
async fn a_file_that_changed_on_the_server_is_not_resumed_by_default() {
    let size = 24 << 20;
    let o = start(Config { size, per_conn_bps: 800_000, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path()).await;
    e.update_settings(serde_json::json!({"defaultStreams": 2, "maxStreams": 2})).unwrap();
    let job = add(&e, o.url("v.bin"));
    wait_for(&e, &job.id, |j| j.downloaded > (1 << 20), 30).await;
    e.pause(&job.id).unwrap();
    wait_for(&e, &job.id, |j| j.status == JobStatus::Paused, 10).await;

    o.cfg.lock().etag = "\"v2\"".into(); // the file is replaced while paused
    e.resume(&job.id).unwrap();
    let j = wait_for(&e, &job.id, |j| j.status == JobStatus::Failed, 20).await;
    assert!(j.error.unwrap().contains("can't be resumed"));
    assert!(!j.resumable);
    // The partial file is still there; nothing was spliced or thrown away behind the user's back.
    assert!(std::fs::read_dir(dir.path().join("dl")).unwrap().any(|f| f.unwrap().file_name().to_string_lossy().ends_with(".idmx")));

    // "Start over" fetches the new version.
    o.cfg.lock().per_conn_bps = 0;
    e.restart(&job.id).unwrap();
    let done = wait_for(&e, &job.id, |j| j.status == JobStatus::Completed, 60).await;
    assert_eq!(std::fs::read(done.path.unwrap()).unwrap(), expected(size));
    e.shutdown().await;
}

#[tokio::test]
async fn with_the_setting_off_a_changed_file_restarts_automatically() {
    let size = 16 << 20;
    let o = start(Config { size, per_conn_bps: 800_000, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path()).await;
    e.update_settings(serde_json::json!({"defaultStreams": 2, "maxStreams": 2, "refuseChangedResume": false})).unwrap();
    let job = add(&e, o.url("v.bin"));
    wait_for(&e, &job.id, |j| j.downloaded > (1 << 20), 30).await;
    e.pause(&job.id).unwrap();
    wait_for(&e, &job.id, |j| j.status == JobStatus::Paused, 10).await;
    o.cfg.lock().etag = "\"v2\"".into();
    o.cfg.lock().per_conn_bps = 0;
    e.resume(&job.id).unwrap();
    let done = wait_for(&e, &job.id, |j| j.status == JobStatus::Completed, 60).await;
    assert!(done.log.iter().any(|l| l.contains("starting over")), "{:?}", done.log);
    assert_eq!(std::fs::read(done.path.unwrap()).unwrap(), expected(size));
    e.shutdown().await;
}

// ───────────── networks coming and going ─────────────

#[tokio::test]
async fn a_network_that_appears_mid_download_joins_switched_off_and_can_be_turned_on() {
    let size = 90 << 20;
    let o = start(Config { size, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let e = engine_with(dir.path(), vec![lo_named("a", "10.1.0.0/24")], |_| {}).await;
    e.update_settings(serde_json::json!({"interfaces": {
        "a": {"metered": false, "capPeriod": "monthly", "speedLimit": 2_000_000},
        "b": {"metered": false, "capPeriod": "monthly", "speedLimit": 2_000_000}
    }}))
    .unwrap();
    let job = add(&e, o.url("big.bin"));
    let j = wait_for(&e, &job.id, |j| j.status == JobStatus::Downloading && j.networks.len() == 1, 20).await;
    assert_eq!(j.networks[0].id, "a");

    // A phone is plugged in.
    let mut ev = e.subscribe();
    e.simulate_networks(vec![lo_named("a", "10.1.0.0/24"), lo_named("b", "10.2.0.0/24")]);
    let j = wait_for(&e, &job.id, |j| j.networks.len() == 2, 5).await;
    let b = j.networks.iter().find(|n| n.id == "b").unwrap();
    assert!(!b.enabled && b.state == NetState::Disabled, "a new network must start off: {b:?}");
    assert_eq!(b.bytes, 0);
    let mut told = false;
    while let Ok(x) = ev.try_recv() {
        told |= matches!(x, Event::Notice { ref message, .. } if message.contains("just connected"));
    }
    assert!(told, "the user should be told it appeared");

    // Turning it on puts it to work straight away.
    e.set_job_network(&job.id, "b", true).unwrap();
    let j = wait_for(&e, &job.id, |j| j.networks.iter().any(|n| n.id == "b" && n.bytes > 0), 15).await;
    assert!(j.networks.iter().find(|n| n.id == "b").unwrap().enabled);
    // Seen once, not re-announced or duplicated on the next change.
    e.simulate_networks(vec![lo_named("a", "10.1.0.0/24"), lo_named("b", "10.2.0.0/24")]);
    assert_eq!(e.get(&job.id).unwrap().networks.len(), 2);
    e.cancel(&job.id).unwrap();
    e.shutdown().await;
}

#[tokio::test]
async fn with_no_network_downloads_wait_instead_of_failing_and_start_when_one_returns() {
    let size = 3 << 20;
    let o = start(Config { size, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let e = engine_with(dir.path(), vec![], |_| {}).await;
    assert!(!e.is_online());
    let job = add(&e, o.url("wait.bin"));
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let j = e.get(&job.id).unwrap();
    assert_eq!(j.status, JobStatus::Queued, "offline downloads must wait, not fail: {:?}", j.error);
    assert!(j.error.is_none());

    e.simulate_networks(vec![lo()]);
    assert!(e.is_online());
    let done = wait_for(&e, &job.id, |j| j.status == JobStatus::Completed, 30).await;
    assert_eq!(std::fs::read(done.path.unwrap()).unwrap(), expected(size));
    e.shutdown().await;
}

#[tokio::test]
async fn a_network_that_drops_out_and_returns_mid_download_is_reported_then_recovers() {
    let size = 60 << 20;
    let o = start(Config { size, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let e = engine_with(dir.path(), vec![lo_named("a", "10.1.0.0/24"), lo_named("b", "10.2.0.0/24")], |_| {}).await;
    e.update_settings(serde_json::json!({"interfaces": {
        "a": {"metered": false, "capPeriod": "monthly", "speedLimit": 2_500_000},
        "b": {"metered": false, "capPeriod": "monthly", "speedLimit": 2_500_000}
    }}))
    .unwrap();
    let job = add(&e, o.url("flaky.bin"));
    wait_for(&e, &job.id, |j| j.networks.len() == 2 && j.networks.iter().all(|n| n.bytes > 0), 20).await;

    e.simulate_networks(vec![lo_named("a", "10.1.0.0/24")]); // b is unplugged
    let j = wait_for(&e, &job.id, |j| j.networks.iter().any(|n| n.id == "b" && n.state == NetState::Offline), 5).await;
    assert!(j.networks.iter().find(|n| n.id == "b").unwrap().enabled, "offline is not the same as switched off");
    assert_eq!(j.status, JobStatus::Downloading, "the other network carries on");

    e.simulate_networks(vec![lo_named("a", "10.1.0.0/24"), lo_named("b", "10.2.0.0/24")]); // and back in
    let j = wait_for(&e, &job.id, |j| j.networks.iter().any(|n| n.id == "b" && n.state == NetState::Active), 10).await;
    assert!(j.networks.iter().find(|n| n.id == "b").unwrap().enabled);
    e.cancel(&job.id).unwrap();
    e.shutdown().await;
}

#[tokio::test]
async fn a_finished_download_keeps_what_the_summary_needs() {
    let size = 14 << 20;
    let o = start(Config { size, ..Default::default() }).await;
    let dir = tempfile::tempdir().unwrap();
    let e = engine_with(dir.path(), vec![lo_named("a", "10.1.0.0/24"), lo_named("b", "10.2.0.0/24")], |_| {}).await;
    e.update_settings(serde_json::json!({"interfaces": {
        "a": {"metered": false, "capPeriod": "monthly", "speedLimit": 1_500_000},
        "b": {"metered": false, "capPeriod": "monthly", "speedLimit": 1_500_000}
    }}))
    .unwrap();
    let job = add(&e, o.url("sum.bin"));
    let done = wait_for(&e, &job.id, |j| j.status == JobStatus::Completed, 40).await;
    assert!(done.peak_streams >= 2, "peak connections should be recorded: {}", done.peak_streams);
    assert!(done.speed_history.len() >= 2, "speed over the download should be kept: {:?}", done.speed_history);
    assert!(done.speed_history.iter().all(|v| *v > 0.0));
    let bytes: u64 = done.networks.iter().map(|n| n.bytes).sum();
    assert!(bytes >= size, "per-network contribution is kept");
    e.shutdown().await;
    drop(e);
    tokio::time::sleep(Duration::from_millis(300)).await;
    // …and it survives a restart.
    let e2 = engine_with(dir.path(), vec![lo_named("a", "10.1.0.0/24")], |_| {}).await;
    let back = e2.get(&job.id).unwrap();
    assert_eq!((back.peak_streams, back.speed_history.len()), (done.peak_streams, done.speed_history.len()));
    e2.shutdown().await;
}
