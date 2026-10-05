//! Videos: a fake yt-dlp points at a local origin, so the whole path — extraction, multi-network
//! download of each stream, merging, filing — runs for real without touching the internet.

mod common;

use common::*;
use idmx_core::types::*;
use idmx_core::{Engine, EngineOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn lo(id: &str) -> NetInterface {
    NetInterface { id: id.into(), label: id.into(), kind: IfKind::Ethernet, ipv4: vec!["127.0.0.1".into()], ipv6: vec![], gateway: None, subnets: vec![], mac: None, up: true, link_mbps: None, usable: true }
}

/// A shell script standing in for yt-dlp. `-J` prints the JSON in `answer.json`; a real download
/// run writes `out_content` as the file and prints progress.
fn fake_ytdlp(dir: &Path, answer: &str) -> PathBuf {
    std::fs::write(dir.join("answer.json"), answer).unwrap();
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "2099.01.01"; exit 0; fi
echo "$@" >> "{dir}/calls.log"
case "$*" in
  *" -J "*) cat "{dir}/answer.json"; exit 0;;
esac
# A real download run: find -P home:… and print progress + the final path.
HOME_DIR=""
for a in "$@"; do case "$a" in home:*) HOME_DIR="${{a#home:}}";; esac; done
mkdir -p "$HOME_DIR"
OUT="$HOME_DIR/Streamed Video [abc].mp4"
i=1
while [ $i -le 5 ]; do
  echo "IDMX|$((i*200000))|1000000|NA|500000|$((5-i))|downloading"
  sleep 0.2
  i=$((i+1))
done
head -c 1000000 /dev/zero > "$OUT"
echo "IDMX|1000000|1000000|NA|0|0|finished"
echo "IDMXFILE|$OUT"
exit 0
"#,
        dir = dir.display()
    );
    let p = dir.join("yt-dlp");
    std::fs::write(&p, script).unwrap();
    set_exec(&p);
    p
}

/// A stand-in for ffmpeg that "merges" by concatenating video then audio.
fn fake_ffmpeg(dir: &Path) -> PathBuf {
    let script = r#"#!/bin/sh
if [ "$1" = "-version" ]; then echo "ffmpeg version 7.0 fake"; exit 0; fi
# args: -y -hide_banner -loglevel error -i V -i A -c copy -map 0:v:0 -map 1:a:0 OUT
V="$6"; A="$8"; for last; do :; done
cat "$V" "$A" > "$last"
"#;
    let p = dir.join("ffmpeg");
    std::fs::write(&p, script).unwrap();
    set_exec(&p);
    p
}

fn set_exec(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

async fn engine(dir: &Path, ytdlp: &Path, ffmpeg: Option<&Path>) -> Engine {
    let e = Engine::start_with(dir.join("data"), EngineOptions { fixed_interfaces: Some(vec![lo("a"), lo("b")]), no_device_binding: true, ..Default::default() }).await.unwrap();
    e.update_settings(serde_json::json!({
        "downloadDir": dir.join("dl").to_string_lossy(),
        "notifications": false,
        "media": { "ytdlpPath": ytdlp.to_string_lossy(), "ffmpegPath": ffmpeg.map(|f| f.to_string_lossy().into_owned()).unwrap_or_default() }
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
        assert!(Instant::now() < end, "timed out; job is {:?} err={:?} media={:?} log={:?}", j.status, j.error, j.media, j.log);
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
}

fn add_media(e: &Engine, url: &str) -> Job {
    e.add(AddRequest { urls: vec![url.into()], options: JobOptions { media: Some(true), ..Default::default() }, ..Default::default() }).unwrap().remove(0)
}

fn pair_json(v: &str, a: &str, vsize: u64, asize: u64) -> String {
    format!(
        r#"{{"id":"abc","title":"My Holiday: Part/1","uploader":"Me","duration":93,"extractor_key":"Youtube","webpage_url":"https://www.youtube.com/watch?v=abc",
        "formats":[{{"height":1080,"vcodec":"avc1"}},{{"height":720,"vcodec":"avc1"}},{{"height":360,"vcodec":"avc1"}},{{"vcodec":"none","acodec":"mp4a"}}],
        "requested_formats":[
          {{"format_id":"137","ext":"mp4","url":"{v}","protocol":"https","height":1080,"vcodec":"avc1","acodec":"none","filesize":{vsize},"http_headers":{{"User-Agent":"FakeUA/1"}}}},
          {{"format_id":"140","ext":"m4a","url":"{a}","protocol":"https","vcodec":"none","acodec":"mp4a","filesize":{asize}}}]}}"#
    )
}

#[tokio::test]
async fn a_video_with_separate_picture_and_sound_is_fetched_over_all_networks_and_joined() {
    let dir = tempfile::tempdir().unwrap();
    let vsize = 12u64 << 20;
    let asize = 2u64 << 20;
    let origin_v = start(Config { size: vsize, ..Default::default() }).await;
    let origin_a = start(Config { size: asize, ..Default::default() }).await;
    let tools = dir.path().join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    let y = fake_ytdlp(&tools, &pair_json(&origin_v.url("video.mp4"), &origin_a.url("audio.m4a"), vsize, asize));
    let f = fake_ffmpeg(&tools);
    let e = engine(dir.path(), &y, Some(&f)).await;

    let job = add_media(&e, "https://www.youtube.com/watch?v=abc");
    assert_eq!(job.kind, JobKind::Media);
    let done = wait_for(&e, &job.id, |j| j.status == JobStatus::Completed, 60).await;

    // Named after the video, with the characters filesystems dislike made safe.
    assert_eq!(done.file_name, "My Holiday_ Part-1.mp4");
    let m = done.media.as_ref().unwrap();
    assert_eq!((m.strategy.as_str(), m.parts, m.format.as_str()), ("direct", 2, "1080p · mp4"));
    assert_eq!(m.title, "My Holiday: Part/1");
    // The joined file is the video then the audio (what the fake ffmpeg does), byte for byte.
    let mut want = expected(vsize);
    want.extend(expected(asize));
    assert_eq!(std::fs::read(done.path.clone().unwrap()).unwrap(), want);
    assert_eq!(done.size, Some(vsize + asize));
    // The headers yt-dlp asked for were used on the requests; the parts and their folder are gone.
    let dl = dir.path().join("dl");
    let left: Vec<String> = std::fs::read_dir(&dl).unwrap().map(|f| f.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(left.len(), 1, "only the finished video should remain: {left:?}");
    // The parts are an implementation detail: they never show up in the list.
    assert_eq!(e.list().len(), 1);
    assert!(e.list().iter().all(|j| j.parent.is_none()));
    // Both networks carried real weight (this is the point).
    assert_eq!(done.networks.len(), 2);
    assert!(done.networks.iter().all(|n| n.bytes > 0), "{:?}", done.networks.iter().map(|n| (&n.id, n.bytes)).collect::<Vec<_>>());
    e.shutdown().await;
}

#[tokio::test]
async fn without_ffmpeg_it_falls_back_to_a_single_file_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let size = 5u64 << 20;
    let origin = start(Config { size, ..Default::default() }).await;
    let tools = dir.path().join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    // First answer (best, needs a merge); the fallback `-f b` answer is a single progressive file.
    let single = format!(
        r#"{{"id":"abc","title":"Single","ext":"mp4","url":"{}","protocol":"https","height":720,"vcodec":"avc1","acodec":"mp4a","filesize":{size}}}"#,
        origin.url("one.mp4")
    );
    // The fake returns the pair for selectors containing "+" and the single file otherwise.
    let y = fake_ytdlp(&tools, &single);
    let script = std::fs::read_to_string(&y).unwrap().replace(
        "  *\" -J \"*) cat \"{dir}/answer.json\"; exit 0;;".replace("{dir}", &tools.display().to_string()).as_str(),
        &format!(
            "  *\" -J \"*) case \"$*\" in *\"+\"*) cat \"{d}/pair.json\";; *) cat \"{d}/answer.json\";; esac; exit 0;;",
            d = tools.display()
        ),
    );
    std::fs::write(&y, script).unwrap();
    std::fs::write(tools.join("pair.json"), pair_json(&origin.url("v.mp4"), &origin.url("a.m4a"), size, size)).unwrap();
    // No ffmpeg anywhere: point at a path that doesn't exist and keep the system's out of reach.
    let e = engine(dir.path(), &y, None).await;
    e.update_settings(serde_json::json!({"media": {"ffmpegPath": "/definitely/not/here/ffmpeg"}})).unwrap();
    // A custom selector that insists on joining two streams, with nothing to join them with.
    let job = e.add(AddRequest { urls: vec!["https://vimeo.com/123".into()], options: JobOptions { media: Some(true), media_format: Some("bv*+ba".into()), ..Default::default() }, ..Default::default() }).unwrap().remove(0);
    let done = wait_for(&e, &job.id, |j| j.status == JobStatus::Completed, 60).await;
    assert_eq!(done.media.as_ref().unwrap().parts, 1);
    assert!(done.log.iter().any(|l| l.contains("ffmpeg wasn't found")), "{:?}", done.log);
    assert_eq!(std::fs::read(done.path.unwrap()).unwrap(), expected(size));
    e.shutdown().await;
}

#[tokio::test]
async fn streaming_formats_are_handed_to_ytdlp_with_live_progress() {
    let dir = tempfile::tempdir().unwrap();
    let tools = dir.path().join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    let hls = r#"{"id":"abc","title":"Live Stream","ext":"mp4","url":"https://cdn.example/master.m3u8","protocol":"m3u8_native","height":1080,"vcodec":"avc1","acodec":"mp4a"}"#;
    let y = fake_ytdlp(&tools, hls);
    let e = engine(dir.path(), &y, None).await;
    let job = add_media(&e, "https://example.com/watch/1");
    let mut saw_progress = false;
    let end = Instant::now() + Duration::from_secs(30);
    let done = loop {
        let j = e.get(&job.id).unwrap();
        saw_progress |= j.status == JobStatus::Downloading && j.downloaded > 0 && j.speed > 0.0;
        if j.status == JobStatus::Completed || j.status == JobStatus::Failed {
            break j;
        }
        assert!(Instant::now() < end, "timed out");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(done.status, JobStatus::Completed, "{:?}", done.error);
    assert!(saw_progress, "progress from yt-dlp should show while it runs");
    assert_eq!(done.media.as_ref().unwrap().strategy, "ytdlp");
    assert!(done.path.unwrap().ends_with("Streamed Video [abc].mp4"));
    // The call carried the options a real run needs.
    let calls = std::fs::read_to_string(tools.join("calls.log")).unwrap();
    assert!(calls.contains("--concurrent-fragments 8") && calls.contains("--newline") && calls.contains("home:"), "{calls}");
    e.shutdown().await;
}

#[tokio::test]
async fn missing_ytdlp_says_how_to_get_it() {
    let dir = tempfile::tempdir().unwrap();
    let e = Engine::start_with(dir.path().join("data"), EngineOptions { fixed_interfaces: Some(vec![lo("a")]), no_device_binding: true, ..Default::default() }).await.unwrap();
    e.update_settings(serde_json::json!({"downloadDir": dir.path().join("dl").to_string_lossy(), "media": {"ytdlpPath": "/nope/yt-dlp"}})).unwrap();
    let job = add_media(&e, "https://www.youtube.com/watch?v=abc");
    let j = wait_for(&e, &job.id, |j| j.status == JobStatus::Failed, 20).await;
    let m = j.error.unwrap();
    assert!(m.contains("yt-dlp") && m.contains("Settings") && m.contains("Install"), "{m}");
    assert_eq!(e.tools_status(true).await.ytdlp.is_none(), true);
    e.shutdown().await;
}

#[tokio::test]
async fn video_site_links_are_recognised_without_being_told() {
    let dir = tempfile::tempdir().unwrap();
    let tools = dir.path().join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    let y = fake_ytdlp(&tools, "{}");
    let e = engine(dir.path(), &y, None).await;
    let j = e.add(AddRequest { urls: vec!["https://www.youtube.com/watch?v=dQw4w9WgXcQ".into()], paused: true, ..Default::default() }).unwrap().remove(0);
    assert_eq!(j.kind, JobKind::Media);
    assert_eq!(j.file_name, "youtube.com · dQw4w9WgXcQ");
    let plain = e.add(AddRequest { urls: vec!["https://example.com/file.zip".into()], paused: true, ..Default::default() }).unwrap().remove(0);
    assert_eq!(plain.kind, JobKind::Http);
    // …unless told to leave it alone.
    let forced = e.add(AddRequest { urls: vec!["https://www.youtube.com/watch?v=x".into()], paused: true, options: JobOptions { force_http: true, ..Default::default() }, ..Default::default() }).unwrap().remove(0);
    assert_eq!(forced.kind, JobKind::Http);
    e.shutdown().await;
}

#[tokio::test]
async fn pausing_a_video_pauses_its_parts_and_cancel_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let size = 40u64 << 20;
    let o1 = start(Config { size, per_conn_bps: 600_000, ..Default::default() }).await;
    let o2 = start(Config { size: 2 << 20, ..Default::default() }).await;
    let tools = dir.path().join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    let y = fake_ytdlp(&tools, &pair_json(&o1.url("v.mp4"), &o2.url("a.m4a"), size, 2 << 20));
    let f = fake_ffmpeg(&tools);
    let e = engine(dir.path(), &y, Some(&f)).await;
    e.update_settings(serde_json::json!({"defaultStreams": 2, "maxStreams": 2})).unwrap();
    let job = add_media(&e, "https://www.youtube.com/watch?v=abc");
    wait_for(&e, &job.id, |j| j.downloaded > (1 << 20), 40).await;

    e.pause(&job.id).unwrap();
    let p = wait_for(&e, &job.id, |j| j.status == JobStatus::Paused, 15).await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(e.get(&job.id).unwrap().downloaded, p.downloaded, "paused means the parts stopped too");

    e.resume(&job.id).unwrap();
    wait_for(&e, &job.id, |j| j.status == JobStatus::Downloading && j.downloaded > p.downloaded, 40).await;

    e.cancel(&job.id).unwrap();
    wait_for(&e, &job.id, |j| j.status == JobStatus::Cancelled, 20).await;
    let dl = dir.path().join("dl");
    let left: Vec<_> = std::fs::read_dir(&dl).map(|d| d.map(|f| f.unwrap().file_name()).collect()).unwrap_or_default();
    assert!(left.is_empty(), "cancel must clean up the hidden part folder: {left:?}");
    assert!(e.list().len() == 1, "no stray parts in the list");
    e.shutdown().await;
}

#[tokio::test]
async fn a_playlist_becomes_one_job_per_video() {
    let dir = tempfile::tempdir().unwrap();
    let tools = dir.path().join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    let pl = r#"{"_type":"playlist","id":"pl","title":"Road Trip","entries":[{"title":"Day 1","url":"https://www.youtube.com/watch?v=1"},{"title":"Day 2","url":"https://www.youtube.com/watch?v=2"},{"title":"Day 3","url":"https://www.youtube.com/watch?v=3"}]}"#;
    let y = fake_ytdlp(&tools, pl);
    let e = engine(dir.path(), &y, None).await;
    e.update_settings(serde_json::json!({"maxConcurrentJobs": 1})).unwrap();
    let job = add_media(&e, "https://www.youtube.com/playlist?list=pl");
    let j = wait_for(&e, &job.id, |j| j.status == JobStatus::Completed, 20).await;
    assert!(j.file_name.contains("Road Trip") && j.file_name.contains("3 videos"), "{}", j.file_name);
    let jobs = e.list();
    assert!(jobs.len() >= 4, "the playlist job plus a job per video: {}", jobs.len());
    assert_eq!(jobs.iter().filter(|j| j.kind == JobKind::Media && j.url.contains("watch?v=")).count(), 3);
    e.shutdown().await;
}

#[tokio::test]
async fn yt_dlp_is_installed_into_idmx_only_if_its_checksum_matches() {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let asset = idmx_core::media::ytdlp_asset().unwrap();
    let body = format!("#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 2099.01.01; fi\n").into_bytes();
    let good = hex::encode(Sha256::digest(&body));

    // A tiny release server.
    async fn serve(files: Vec<(String, Vec<u8>)>) -> std::net::SocketAddr {
        use axum::{routing::get, Router};
        let mut app = Router::new();
        for (name, data) in files {
            app = app.route(&format!("/{name}"), get(move || { let d = data.clone(); async move { d } }));
        }
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let a = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        a
    }
    let ok = serve(vec![(asset.to_string(), body.clone()), ("SHA2-256SUMS".into(), format!("{good}  {asset}\n").into_bytes())]).await;
    let e = Engine::start_with(dir.path().join("data"), EngineOptions { fixed_interfaces: Some(vec![lo("a")]), no_device_binding: true, ytdlp_base: Some(format!("http://{ok}")), ..Default::default() }).await.unwrap();
    assert!(e.tools_status(true).await.ytdlp.is_none());
    let mut ev = e.subscribe();
    e.install_tool("yt-dlp").unwrap();
    let end = Instant::now() + Duration::from_secs(20);
    let mut installed = false;
    while Instant::now() < end && !installed {
        if let Ok(Ok(Event::Tools(t))) = tokio::time::timeout(Duration::from_millis(500), ev.recv()).await {
            installed = t.ytdlp.as_ref().is_some_and(|y| y.managed && y.version == "2099.01.01") && t.installing.is_none();
        }
    }
    assert!(installed, "yt-dlp should be installed and reported");
    let st = e.tools_status(false).await;
    assert!(st.ytdlp.unwrap().path.contains("bin"));
    e.shutdown().await;
    drop(e);

    // A tampered download is refused and nothing is left behind.
    let dir2 = tempfile::tempdir().unwrap();
    let bad = serve(vec![(asset.to_string(), body.clone()), ("SHA2-256SUMS".into(), format!("{}  {asset}\n", "0".repeat(64)).into_bytes())]).await;
    let e2 = Engine::start_with(dir2.path().join("data"), EngineOptions { fixed_interfaces: Some(vec![lo("a")]), no_device_binding: true, ytdlp_base: Some(format!("http://{bad}")), ..Default::default() }).await.unwrap();
    let mut ev = e2.subscribe();
    e2.install_tool("yt-dlp").unwrap();
    let mut err = None;
    let end = Instant::now() + Duration::from_secs(20);
    while Instant::now() < end && err.is_none() {
        if let Ok(Ok(Event::Tools(t))) = tokio::time::timeout(Duration::from_millis(500), ev.recv()).await {
            err = t.installing.and_then(|i| i.error);
        }
    }
    let m = err.expect("a bad checksum must be reported");
    assert!(m.contains("checksum"), "{m}");
    assert!(e2.tools_status(true).await.ytdlp.is_none());
    assert!(!dir2.path().join("data/bin").join(asset).exists());
    e2.shutdown().await;
}
