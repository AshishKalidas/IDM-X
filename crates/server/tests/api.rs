use idmx_core::{Engine, EngineOptions};
use idmx_server::{runtime_info, serve, ServeConfig};
use serde_json::{json, Value};

async fn boot(dir: &std::path::Path) -> (Engine, idmx_server::ServerHandle, reqwest::Client, String) {
    let e = Engine::start_with(dir.join("data"), EngineOptions { fixed_interfaces: Some(vec![]), no_device_binding: true, ..Default::default() }).await.unwrap();
    e.update_settings(json!({"downloadDir": dir.join("dl").to_string_lossy(), "notifications": false})).unwrap();
    let h = serve(e.clone(), ServeConfig::default()).await.unwrap();
    let base = format!("http://127.0.0.1:{}", h.addr.port());
    (e, h, reqwest::Client::new(), base)
}

#[tokio::test]
async fn token_is_required_everywhere_but_health() {
    let dir = tempfile::tempdir().unwrap();
    let (_e, h, c, base) = boot(dir.path()).await;

    assert_eq!(c.get(format!("{base}/api/health")).send().await.unwrap().status(), 200);
    assert_eq!(c.get(format!("{base}/api/state")).send().await.unwrap().status(), 401);
    assert_eq!(c.get(format!("{base}/api/state")).bearer_auth("wrong").send().await.unwrap().status(), 401);
    assert_eq!(c.get(format!("{base}/api/state")).bearer_auth(&h.token).send().await.unwrap().status(), 200);
    assert_eq!(c.get(format!("{base}/api/state?token={}", h.token)).send().await.unwrap().status(), 200);
    assert_eq!(c.get(format!("{base}/api/state")).header("x-idmx-token", &h.token).send().await.unwrap().status(), 200);
    // Mutations are protected too.
    assert_eq!(c.post(format!("{base}/api/jobs")).json(&json!({"urls": ["http://x/y"]})).send().await.unwrap().status(), 401);
    assert_eq!(c.post(format!("{base}/api/pause-all")).send().await.unwrap().status(), 401);
}

#[tokio::test]
async fn cross_origin_pages_cannot_talk_to_it_but_extensions_can() {
    let dir = tempfile::tempdir().unwrap();
    let (_e, _h, c, base) = boot(dir.path()).await;
    let preflight = |origin: &str| {
        c.request(reqwest::Method::OPTIONS, format!("{base}/api/jobs"))
            .header("origin", origin)
            .header("access-control-request-method", "POST")
            .header("access-control-request-headers", "authorization,content-type")
            .send()
    };
    let evil = preflight("https://evil.example").await.unwrap();
    assert!(evil.headers().get("access-control-allow-origin").is_none(), "a random website was allowed");
    let ext = preflight("chrome-extension://abcdef").await.unwrap();
    assert_eq!(ext.headers().get("access-control-allow-origin").unwrap(), "chrome-extension://abcdef");
}

#[tokio::test]
async fn job_lifecycle_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let (_e, h, c, base) = boot(dir.path()).await;
    let auth = |r: reqwest::RequestBuilder| r.bearer_auth(&h.token);

    let r = auth(c.post(format!("{base}/api/jobs"))).json(&json!({"urls": ["http://127.0.0.1:9/f[1-3].bin"], "paused": true})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let jobs: Vec<Value> = r.json().await.unwrap();
    assert_eq!(jobs.len(), 3);
    assert!(jobs.iter().all(|j| j["status"] == "paused"));
    let id = jobs[0]["id"].as_str().unwrap().to_string();

    let state: Value = auth(c.get(format!("{base}/api/state"))).send().await.unwrap().json().await.unwrap();
    assert_eq!(state["jobs"].as_array().unwrap().len(), 3);
    assert_eq!(state["local"], true);

    let patched: Value = auth(c.patch(format!("{base}/api/jobs/{id}"))).json(&json!({"priority": 5})).send().await.unwrap().json().await.unwrap();
    assert_eq!(patched["priority"], 5);

    assert_eq!(auth(c.get(format!("{base}/api/jobs/nope"))).send().await.unwrap().status(), 404);
    assert_eq!(auth(c.delete(format!("{base}/api/jobs/{id}"))).send().await.unwrap().status(), 200);
    assert_eq!(auth(c.get(format!("{base}/api/jobs/{id}"))).send().await.unwrap().status(), 404);

    // Bad input is a clean 400 with a message, never a 500.
    let bad = auth(c.post(format!("{base}/api/jobs"))).json(&json!({"urls": ["definitely not a url"]})).send().await.unwrap();
    assert_eq!(bad.status(), 400);
    let body: Value = bad.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("web address"));
    let junk = auth(c.post(format!("{base}/api/jobs"))).header("content-type", "application/json").body("{nope").send().await.unwrap();
    assert!(junk.status().is_client_error());
}

#[tokio::test]
async fn settings_and_interface_rules_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let (e, h, c, base) = boot(dir.path()).await;
    let auth = |r: reqwest::RequestBuilder| r.bearer_auth(&h.token);

    let s: Value = auth(c.put(format!("{base}/api/settings"))).json(&json!({"maxConcurrentJobs": 5, "globalSpeedLimit": 1000000})).send().await.unwrap().json().await.unwrap();
    assert_eq!(s["maxConcurrentJobs"], 5);
    assert_eq!(e.settings().global_speed_limit, Some(1_000_000));

    let s: Value = auth(c.put(format!("{base}/api/interfaces/en9/rule"))).json(&json!({"metered": true, "dataCap": 5000, "capPeriod": "daily"})).send().await.unwrap().json().await.unwrap();
    assert_eq!(s["interfaces"]["en9"]["metered"], true);
    let s: Value = auth(c.delete(format!("{base}/api/interfaces/en9/rule"))).send().await.unwrap().json().await.unwrap();
    assert!(s["interfaces"].get("en9").is_none());

    // Out-of-range values are clamped rather than trusted.
    let s: Value = auth(c.put(format!("{base}/api/settings"))).json(&json!({"maxConcurrentJobs": 9999})).send().await.unwrap().json().await.unwrap();
    assert_eq!(s["maxConcurrentJobs"], 16);
}

#[tokio::test]
async fn event_stream_opens_with_a_snapshot() {
    use futures_util::StreamExt;
    let dir = tempfile::tempdir().unwrap();
    let (_e, h, c, base) = boot(dir.path()).await;
    let resp = c.get(format!("{base}/api/events?token={}", h.token)).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.headers()["content-type"].to_str().unwrap().starts_with("text/event-stream"));
    let mut s = resp.bytes_stream();
    let first = tokio::time::timeout(std::time::Duration::from_secs(3), s.next()).await.unwrap().unwrap().unwrap();
    let text = String::from_utf8_lossy(&first);
    assert!(text.contains("event: snapshot"), "{text}");
    assert!(text.contains("\"jobs\""));
    // …and live ticks follow.
    let mut saw_tick = false;
    for _ in 0..4 {
        if let Ok(Some(Ok(b))) = tokio::time::timeout(std::time::Duration::from_secs(2), s.next()).await {
            if String::from_utf8_lossy(&b).contains("\"type\":\"tick\"") {
                saw_tick = true;
                break;
            }
        }
    }
    assert!(saw_tick, "no tick arrived");
}

#[tokio::test]
async fn runtime_file_lets_other_processes_find_us_and_is_cleaned_up() {
    let dir = tempfile::tempdir().unwrap();
    let (e, h, _c, _base) = boot(dir.path()).await;
    let info = runtime_info(&e.data_dir()).expect("runtime file should exist");
    assert_eq!(info.port, h.addr.port());
    assert_eq!(info.token, h.token);
    drop(h);
    assert!(!e.data_dir().join("runtime.json").exists());
    assert!(runtime_info(&e.data_dir()).is_none());
}

#[tokio::test]
async fn new_endpoints_validate_input_and_report_what_they_find() {
    let dir = tempfile::tempdir().unwrap();
    let (_e, h, c, base) = boot(dir.path()).await;
    let auth = |r: reqwest::RequestBuilder| r.bearer_auth(&h.token);

    let jobs: Vec<Value> = auth(c.post(format!("{base}/api/jobs"))).json(&json!({"urls": ["http://127.0.0.1:9/a.bin"], "paused": true})).send().await.unwrap().json().await.unwrap();
    let id = jobs[0]["id"].as_str().unwrap();

    // Not running: no connections, but it's a valid answer, not an error.
    let streams: Vec<Value> = auth(c.get(format!("{base}/api/jobs/{id}/streams"))).send().await.unwrap().json().await.unwrap();
    assert!(streams.is_empty());
    assert_eq!(auth(c.get(format!("{base}/api/jobs/nope/streams"))).send().await.unwrap().status(), 404);

    // Switching a network on a download that isn't running just records the choice.
    let r = auth(c.post(format!("{base}/api/jobs/{id}/networks/en0"))).json(&json!({"enabled": false})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let j: Value = auth(c.get(format!("{base}/api/jobs/{id}"))).send().await.unwrap().json().await.unwrap();
    assert_eq!(j["options"]["interfaces"], json!([]));
    assert_eq!(auth(c.post(format!("{base}/api/jobs/nope/networks/en0"))).json(&json!({"enabled": true})).send().await.unwrap().status(), 404);
    assert!(auth(c.post(format!("{base}/api/jobs/{id}/networks/en0"))).json(&json!({"wrong": 1})).send().await.unwrap().status().is_client_error());

    // Updates: nothing configured → null, and the snapshot carries it too.
    let u: Value = auth(c.get(format!("{base}/api/update"))).send().await.unwrap().json().await.unwrap();
    assert!(u.is_null());
    let chk: Value = auth(c.post(format!("{base}/api/update/check"))).send().await.unwrap().json().await.unwrap();
    assert!(chk.is_null());
    let st: Value = auth(c.get(format!("{base}/api/state"))).send().await.unwrap().json().await.unwrap();
    assert!(st.get("update").is_some());
    // A bad source is reported, not swallowed.
    auth(c.put(format!("{base}/api/settings"))).json(&json!({"updateRepo": "not a repo"})).send().await.unwrap();
    let bad = auth(c.post(format!("{base}/api/update/check"))).send().await.unwrap();
    assert!(bad.status().is_client_error() || bad.status().is_server_error());

    // Ping works with no networks (empty list) and requires auth like everything else.
    let p: Vec<Value> = auth(c.post(format!("{base}/api/ping"))).send().await.unwrap().json().await.unwrap();
    assert!(p.is_empty());
    assert_eq!(c.post(format!("{base}/api/ping")).send().await.unwrap().status(), 401);
}

#[tokio::test]
async fn errors_come_back_as_plain_sentences_with_a_next_step() {
    let dir = tempfile::tempdir().unwrap();
    let (_e, h, c, base) = boot(dir.path()).await;
    let auth = |r: reqwest::RequestBuilder| r.bearer_auth(&h.token);
    let err = |r: reqwest::Response| async move { r.json::<Value>().await.unwrap()["error"].as_str().unwrap().to_string() };

    let r = auth(c.post(format!("{base}/api/probe"))).json(&json!({"url": "http://127.0.0.1:9/x"})).send().await.unwrap();
    assert!(r.status().is_client_error() || r.status().is_server_error());
    let m = err(r).await;
    assert!(m.contains("refused the connection") && m.contains("server may be down"), "{m}");

    let r = auth(c.post(format!("{base}/api/probe"))).json(&json!({"url": "http://no-such-host.invalid/x"})).send().await.unwrap();
    let m = err(r).await;
    assert!(m.contains("Couldn't find") && m.contains("typos"), "{m}");

    for (bad, needle) in [("example.com/file.zip", "http://"), ("ftp://example.com/x", "isn't supported"), ("", "empty")] {
        let r = auth(c.post(format!("{base}/api/probe"))).json(&json!({"url": bad})).send().await.unwrap();
        assert_eq!(r.status(), 400, "{bad}");
        let m = err(r).await;
        assert!(m.contains(needle), "{bad} → {m}");
    }

    // The refresh endpoint exists, is authenticated, and returns the networks.
    assert_eq!(c.post(format!("{base}/api/networks/refresh")).send().await.unwrap().status(), 401);
    let list: Vec<Value> = auth(c.post(format!("{base}/api/networks/refresh"))).send().await.unwrap().json().await.unwrap();
    assert!(list.is_empty());
}

#[tokio::test]
async fn tools_media_and_torrent_endpoints_work_and_stay_local_where_they_must() {
    let dir = tempfile::tempdir().unwrap();
    let (e, h, c, base) = boot(dir.path()).await;
    let auth = |r: reqwest::RequestBuilder| r.bearer_auth(&h.token);

    // Tools: nothing is installed in a fresh data folder; the answer says so rather than failing.
    e.update_settings(json!({"media": {"ytdlpPath": "/nope/yt-dlp", "ffmpegPath": "/nope/ffmpeg", "jsRuntimePath": "/nope/deno"}})).unwrap();
    let t: Value = auth(c.get(format!("{base}/api/tools"))).send().await.unwrap().json().await.unwrap();
    assert!(t["installing"].is_null());
    let t: Value = auth(c.post(format!("{base}/api/tools/refresh"))).send().await.unwrap().json().await.unwrap();
    assert!(t["ytdlp"].is_null() && t["ffmpeg"].is_null());
    // Unknown tool names are a clean 400.
    let r = auth(c.post(format!("{base}/api/tools/install/launch-missiles"))).send().await.unwrap();
    assert_eq!(r.status(), 400);

    // A video probe with no yt-dlp tells you how to get it.
    let r = auth(c.post(format!("{base}/api/media/probe"))).json(&json!({"url": "https://www.youtube.com/watch?v=abc"})).send().await.unwrap();
    assert!(r.status().is_client_error());
    let m = r.json::<Value>().await.unwrap()["error"].as_str().unwrap().to_string();
    assert!(m.contains("yt-dlp") && m.contains("Install"), "{m}");

    // A torrent probe of garbage is explained.
    let r = auth(c.post(format!("{base}/api/torrent/inspect"))).json(&json!({"url": "hello"})).send().await.unwrap();
    assert_eq!(r.status(), 400);
    assert!(r.json::<Value>().await.unwrap()["error"].as_str().unwrap().contains("magnet"));

    // Adding a magnet link makes a torrent job (paused here so nothing goes out on the network).
    let jobs: Vec<Value> = auth(c.post(format!("{base}/api/jobs"))).json(&json!({"urls": ["magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Demo+Files"], "paused": true})).send().await.unwrap().json().await.unwrap();
    assert_eq!(jobs[0]["kind"], "torrent");
    assert_eq!(jobs[0]["fileName"], "Demo Files");
    let id = jobs[0]["id"].as_str().unwrap();
    let r = auth(c.post(format!("{base}/api/jobs/{id}/torrent-files"))).json(&json!({"files": [0, 2]})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let j: Value = auth(c.get(format!("{base}/api/jobs/{id}"))).send().await.unwrap().json().await.unwrap();
    assert_eq!(j["options"]["torrentFiles"], json!([0, 2]));

    // The extension saying hello shows up in the state, and in events for the UI.
    let r: Value = auth(c.post(format!("{base}/api/extension/hello"))).json(&json!({"version": "0.2.0", "browser": "Chrome"})).send().await.unwrap().json().await.unwrap();
    assert_eq!(r["ok"], true);
    let st: Value = auth(c.get(format!("{base}/api/state"))).send().await.unwrap().json().await.unwrap();
    assert_eq!(st["extension"]["browser"], "Chrome");
    assert_eq!(st["extension"]["version"], "0.2.0");

    // Settings that run programs or open ports are local-only; they're all editable here (loopback).
    let s: Value = auth(c.put(format!("{base}/api/settings"))).json(&json!({"torrent": {"seed": false, "seedRatio": 2.5}})).send().await.unwrap().json().await.unwrap();
    assert_eq!(s["torrent"]["seed"], false);
    assert_eq!(s["torrent"]["seedRatio"], 2.5);
    assert_eq!(s["media"]["defaultQuality"], "best");
}
