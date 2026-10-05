//! Video and audio pages, via yt-dlp. IDMX asks yt-dlp *what* to download (it knows how to find the
//! real stream on hundreds of sites) and then does the downloading itself, over all your networks,
//! whenever the stream is a plain file. Streaming formats are handed to yt-dlp's own downloader.

use crate::error::{Error, Result};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

// ───────────────────────────── tools ─────────────────────────────

#[derive(Clone, Debug)]
pub struct Tool {
    pub path: PathBuf,
    pub version: String,
    /// Installed and kept up to date by IDMX (as opposed to found on the system).
    pub managed: bool,
}

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

pub fn bin_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("bin")
}

/// Look for a tool: the path the user gave, the copy IDMX installed, then the system.
pub fn find_tool(name: &str, configured: &str, data_dir: &Path) -> Option<(PathBuf, bool)> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let p = PathBuf::from(configured);
        return p.is_file().then_some((p, false));
    }
    let managed = bin_dir(data_dir).join(exe(name));
    if managed.is_file() {
        return Some((managed, true));
    }
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    // A GUI app on macOS doesn't inherit the shell's PATH, so look where Homebrew puts things.
    for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/snap/bin", "/home/linuxbrew/.linuxbrew/bin"] {
        dirs.push(PathBuf::from(extra));
    }
    dirs.into_iter().map(|d| d.join(exe(name))).find(|p| p.is_file()).map(|p| (p, false))
}

pub async fn tool_version(path: &Path, flag: &str) -> Option<String> {
    let out = tokio::time::timeout(Duration::from_secs(15), Command::new(path).arg(flag).kill_on_drop(true).output()).await.ok()?.ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let first = text.lines().next()?.trim().to_string();
    out.status.success().then_some(first).filter(|s| !s.is_empty())
}

pub async fn detect(name: &str, configured: &str, data_dir: &Path) -> Option<Tool> {
    let (path, managed) = find_tool(name, configured, data_dir)?;
    let flag = if name == "ffmpeg" { "-version" } else { "--version" };
    let v = tool_version(&path, flag).await?;
    let version = if name == "ffmpeg" { v.split_whitespace().nth(2).unwrap_or(&v).to_string() } else { v };
    Some(Tool { path, version, managed })
}

/// Which file on yt-dlp's releases page is the standalone program for this machine.
pub fn ytdlp_asset() -> Result<&'static str> {
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", _) => "yt-dlp_macos",
        ("windows", "x86_64") => "yt-dlp.exe",
        ("windows", "aarch64") => "yt-dlp_arm64.exe",
        ("windows", _) => "yt-dlp_x86.exe",
        ("linux", "x86_64") => "yt-dlp_linux",
        ("linux", "aarch64") => "yt-dlp_linux_aarch64",
        (os, arch) => return Err(Error::msg(format!("There's no ready-made yt-dlp for {os}/{arch}. Install it yourself and set its path in Settings."))),
    })
}

/// The expected SHA-256 of `asset`, out of a `SHA2-256SUMS` file.
pub fn expected_sha(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let mut p = l.split_whitespace();
        let (h, n) = (p.next()?, p.next()?.trim_start_matches('*'));
        (n == asset).then(|| h.to_ascii_lowercase())
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolKind {
    YtDlp,
    /// A JavaScript runtime. YouTube now makes yt-dlp run a little JavaScript to unlock the full-quality formats.
    Deno,
    Ffmpeg,
}

impl ToolKind {
    pub fn name(self) -> &'static str {
        match self {
            ToolKind::YtDlp => "yt-dlp",
            ToolKind::Deno => "deno",
            ToolKind::Ffmpeg => "ffmpeg",
        }
    }
    pub fn parse(s: &str) -> Option<ToolKind> {
        match s {
            "ytdlp" | "yt-dlp" => Some(ToolKind::YtDlp),
            "deno" | "js" => Some(ToolKind::Deno),
            "ffmpeg" => Some(ToolKind::Ffmpeg),
            _ => None,
        }
    }
    pub fn default_base(self) -> &'static str {
        match self {
            ToolKind::YtDlp => "https://github.com/yt-dlp/yt-dlp/releases/latest/download",
            ToolKind::Deno => "https://github.com/denoland/deno/releases/latest/download",
            ToolKind::Ffmpeg => "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1",
        }
    }
}

/// The deno release file for this machine.
pub fn deno_asset() -> Result<&'static str> {
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "deno-aarch64-apple-darwin.zip",
        ("macos", _) => "deno-x86_64-apple-darwin.zip",
        ("linux", "aarch64") => "deno-aarch64-unknown-linux-gnu.zip",
        ("linux", _) => "deno-x86_64-unknown-linux-gnu.zip",
        ("windows", _) => "deno-x86_64-pc-windows-msvc.zip",
        (os, arch) => return Err(Error::msg(format!("There's no ready-made JavaScript runtime for {os}/{arch}."))),
    })
}

/// The ffmpeg build for this machine and the SHA-256 of that exact file. These are pinned: the
/// download is only trusted if it is byte-for-byte the build we checked.
pub fn ffmpeg_asset() -> Result<(&'static str, &'static str)> {
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => ("ffmpeg-darwin-arm64.gz", "8923876afa8db5585022d7860ec7e589af192f441c56793971276d450ed3bbfa"),
        ("macos", _) => ("ffmpeg-darwin-x64.gz", "929b375c1182d956c51f7ac25e0b2b0411fb01f6f407aa15c9758efeb4242106"),
        ("linux", "aarch64") => ("ffmpeg-linux-arm64.gz", "754a678672298bc68156adff58aa7385a592c2b30b1d0ae8750c45c915c4bac0"),
        ("linux", _) => ("ffmpeg-linux-x64.gz", "bfe8a8fc511530457b528c48d77b5737527b504a3797a9bc4866aeca69c2dffa"),
        ("windows", _) => ("ffmpeg-win32-x64.gz", "8883a3dffbd0a16cf4ef95206ea05283f78908dbfb118f73c83f4951dcc06d77"),
        (os, arch) => return Err(Error::msg(format!("There's no ready-made ffmpeg for {os}/{arch}. Install it yourself and set its path in Settings."))),
    })
}

/// Download `kind` into IDMX's own folder and check it against a published (or pinned) checksum
/// before it is trusted. `base` is where the release files live (overridable for mirrors and tests).
pub async fn install(kind: ToolKind, http: &reqwest::Client, data_dir: &Path, base: Option<&str>, progress: impl Fn(u64, Option<u64>)) -> Result<Tool> {
    use futures_util::StreamExt;
    let overridden = base.is_some();
    let base = base.unwrap_or(kind.default_base());
    let (asset, pinned): (String, Option<String>) = match kind {
        ToolKind::YtDlp => (ytdlp_asset()?.to_string(), None),
        ToolKind::Deno => (deno_asset()?.to_string(), None),
        ToolKind::Ffmpeg => {
            let (a, h) = ffmpeg_asset()?;
            (a.to_string(), (!overridden).then(|| h.to_string()))
        }
    };
    let dir = bin_dir(data_dir);
    std::fs::create_dir_all(&dir)?;

    // The checksum we'll hold the download to.
    let want = match pinned {
        Some(h) => Some(h),
        None => {
            let (sums_url, parse): (String, fn(&str, &str) -> Option<String>) = match kind {
                ToolKind::YtDlp => (format!("{base}/SHA2-256SUMS"), expected_sha),
                _ => (format!("{base}/{asset}.sha256sum"), expected_sha_single),
            };
            match http.get(sums_url).timeout(Duration::from_secs(30)).send().await {
                Ok(r) if r.status().is_success() => r.text().await.ok().and_then(|t| parse(&t, &asset)),
                _ => None,
            }
        }
    };
    let Some(want) = want else {
        return Err(Error::msg(format!("Couldn't get {}'s published checksum, so IDMX won't install it (it can't be verified). Check your connection and try again.", kind.name())));
    };

    let resp = http.get(format!("{base}/{asset}")).send().await?;
    if !resp.status().is_success() {
        return Err(Error::Http(resp.status().as_u16()));
    }
    let total = resp.content_length();
    let tmp = dir.join(format!("{asset}.download"));
    let mut file = tokio::fs::File::create(&tmp).await?;
    let mut hasher = Sha256::new();
    let mut got = 0u64;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        hasher.update(&chunk);
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        got += chunk.len() as u64;
        progress(got, total);
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    drop(file);
    if hex::encode(hasher.finalize()) != want {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::msg(format!("The download of {} didn't match its published checksum, so it was thrown away. Try again.", kind.name())));
    }

    // Unpack if needed, then put it in place.
    let dest = dir.join(exe(kind.name()));
    let staged = dir.join(format!("{}.staged", kind.name()));
    match kind {
        ToolKind::YtDlp => std::fs::rename(&tmp, &staged)?,
        ToolKind::Ffmpeg => {
            let (src, out) = (tmp.clone(), staged.clone());
            tokio::task::spawn_blocking(move || -> Result<()> {
                let mut gz = flate2::read::GzDecoder::new(std::fs::File::open(&src)?);
                std::io::copy(&mut gz, &mut std::fs::File::create(&out)?)?;
                let _ = std::fs::remove_file(&src);
                Ok(())
            })
            .await
            .map_err(|e| Error::msg(e.to_string()))??;
        }
        ToolKind::Deno => {
            let (src, out) = (tmp.clone(), staged.clone());
            tokio::task::spawn_blocking(move || -> Result<()> {
                let mut z = zip::ZipArchive::new(std::fs::File::open(&src)?).map_err(|e| Error::msg(format!("The deno download isn't a valid archive: {e}")))?;
                let idx = (0..z.len()).find(|i| z.by_index(*i).map(|f| matches!(f.name(), "deno" | "deno.exe")).unwrap_or(false)).ok_or_else(|| Error::msg("The deno download didn't contain the program."))?;
                let mut entry = z.by_index(idx).map_err(|e| Error::msg(e.to_string()))?;
                std::io::copy(&mut entry, &mut std::fs::File::create(&out)?)?;
                let _ = std::fs::remove_file(&src);
                Ok(())
            })
            .await
            .map_err(|e| Error::msg(e.to_string()))??;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("xattr").args(["-d", "com.apple.quarantine"]).arg(&staged).output();
    std::fs::rename(&staged, &dest)?;
    let flag = if kind == ToolKind::Ffmpeg { "-version" } else { "--version" };
    let version = tool_version(&dest, flag).await.ok_or_else(|| Error::msg(format!("{} was installed but won't start on this computer.", kind.name())))?;
    let version = if kind == ToolKind::Ffmpeg { version.split_whitespace().nth(2).unwrap_or(&version).to_string() } else { version };
    Ok(Tool { path: dest, version, managed: true })
}

/// A `.sha256sum` file for one asset: `<hash>  <name>` (or just the hash).
pub fn expected_sha_single(text: &str, _asset: &str) -> Option<String> {
    text.split_whitespace().next().map(|h| h.to_ascii_lowercase()).filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
}

/// A JavaScript runtime yt-dlp can use: `(name yt-dlp knows it by, path)`.
pub fn find_js_runtime(configured: &str, data_dir: &Path) -> Option<(String, PathBuf)> {
    if !configured.trim().is_empty() {
        let p = PathBuf::from(configured.trim());
        let stem = p.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let name = if stem.starts_with("node") { "node" } else if stem.starts_with("bun") { "bun" } else if stem.starts_with("qjs") || stem.starts_with("quickjs") { "quickjs" } else { "deno" };
        return p.is_file().then(|| (name.to_string(), p));
    }
    for name in ["deno", "node", "bun"] {
        if let Some((p, _)) = find_tool(name, "", data_dir) {
            return Some((name.to_string(), p));
        }
    }
    find_tool("qjs", "", data_dir).map(|(p, _)| ("quickjs".to_string(), p))
}

// ───────────────────────────── what to fetch ─────────────────────────────

/// yt-dlp format selectors for the presets in the UI.
/// `audio216` / `audio320`: save as an MP3 at that bitrate (needs ffmpeg).
pub fn audio_bitrate(preset: &str) -> Option<u32> {
    preset.trim().strip_prefix("audio").and_then(|n| n.parse().ok()).filter(|n| *n > 0)
}

/// Convert an audio file to MP3 at `kbps`.
pub async fn to_mp3(ffmpeg: &Path, input: &Path, out: &Path, kbps: u32) -> Result<()> {
    let output = Command::new(ffmpeg)
        .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(input)
        .args(["-vn", "-codec:a", "libmp3lame", "-b:a"])
        .arg(format!("{kbps}k"))
        .arg(out)
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| Error::msg(format!("Couldn't start ffmpeg: {e}")))?;
    if !output.status.success() {
        let e = String::from_utf8_lossy(&output.stderr);
        return Err(Error::msg(format!("ffmpeg couldn't convert the audio: {}", e.lines().last().unwrap_or("unknown error"))));
    }
    Ok(())
}

pub fn selector_for(preset: &str, can_merge: bool) -> String {
    let h = |n: u32| {
        if can_merge {
            format!("bv*[height<={n}]+ba/b[height<={n}]/b")
        } else {
            // Without ffmpeg only a single file with sound in it will do.
            format!("b[height<={n}]/b")
        }
    };
    match preset.trim() {
        "" | "best" => {
            if can_merge { "bv*+ba/b".into() } else { "b".into() }
        }
        "2160" | "1440" | "1080" | "720" | "480" | "360" => h(preset.trim().parse().unwrap_or(1080)),
        "audio" | "audio216" | "audio320" => "ba[ext=m4a]/ba/b".into(),
        // Anything else is taken to be yt-dlp's own selector.
        custom => custom.to_string(),
    }
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct RawFormat {
    #[serde(default)]
    pub format_id: String,
    #[serde(default)]
    pub ext: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub vcodec: Option<String>,
    #[serde(default)]
    pub acodec: Option<String>,
    #[serde(default)]
    pub filesize: Option<u64>,
    #[serde(default)]
    pub filesize_approx: Option<f64>,
    #[serde(default)]
    pub format_note: Option<String>,
    #[serde(default)]
    pub http_headers: HashMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct Part {
    pub format_id: String,
    pub ext: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub size: Option<u64>,
    pub has_video: bool,
    pub has_audio: bool,
    /// A plain file we can fetch with ranges (as opposed to HLS/DASH).
    pub direct: bool,
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct Extracted {
    pub id: String,
    pub title: String,
    pub uploader: Option<String>,
    pub thumbnail: Option<String>,
    pub duration: Option<u64>,
    pub extractor: Option<String>,
    pub page_url: String,
    pub parts: Vec<Part>,
    pub is_playlist: bool,
    pub playlist_entries: Vec<(String, String)>,
    /// Video heights the page offers (for the quality picker), tallest first.
    pub heights: Vec<u32>,
}

impl Extracted {
    /// Every chosen part is a plain file: IDMX can fetch the whole thing itself.
    pub fn all_direct(&self) -> bool {
        !self.parts.is_empty() && self.parts.iter().all(|p| p.direct)
    }
    pub fn needs_merge(&self) -> bool {
        self.parts.len() > 1
    }
    pub fn label(&self) -> String {
        let video = self.parts.iter().find(|p| p.has_video);
        let ext = self.parts.first().map(|p| p.ext.as_str()).unwrap_or("");
        match video {
            Some(v) => format!("{}{}{}", v.label, if v.label.is_empty() { "" } else { " · " }, if self.needs_merge() { merged_ext(&self.parts) } else { ext }),
            None => format!("audio · {ext}"),
        }
    }
}

pub fn parse_extracted(json: &str, page_url: &str) -> Result<Extracted> {
    let v: Value = serde_json::from_str(json).map_err(|_| Error::msg("yt-dlp's answer couldn't be read. Update yt-dlp (Settings → Video downloads) and try again."))?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let kind = s("_type").unwrap_or_else(|| "video".into());
    let title = s("title").unwrap_or_else(|| "video".into());
    let base = |parts: Vec<Part>, is_playlist: bool, entries: Vec<(String, String)>| Extracted {
        id: s("id").unwrap_or_default(),
        title: title.clone(),
        uploader: s("uploader").or_else(|| s("channel")),
        thumbnail: s("thumbnail"),
        duration: v.get("duration").and_then(|d| d.as_f64()).map(|d| d as u64),
        extractor: s("extractor_key").or_else(|| s("extractor")),
        page_url: s("webpage_url").unwrap_or_else(|| page_url.to_string()),
        parts,
        is_playlist,
        playlist_entries: entries,
        heights: {
            let mut h: Vec<u32> = v.get("formats").and_then(|f| f.as_array()).map(|a| a.iter().filter(|f| f.get("vcodec").and_then(|c| c.as_str()).is_some_and(|c| c != "none")).filter_map(|f| f.get("height").and_then(|h| h.as_u64()).map(|h| h as u32)).collect()).unwrap_or_default();
            h.sort_unstable_by(|a, b| b.cmp(a));
            h.dedup();
            h
        },
    };
    if kind == "playlist" || kind == "multi_video" {
        let entries: Vec<(String, String)> = v
            .get("entries")
            .and_then(|e| e.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|e| {
                        let url = e.get("webpage_url").or_else(|| e.get("url")).and_then(|u| u.as_str())?.to_string();
                        let t = e.get("title").and_then(|t| t.as_str()).unwrap_or("video").to_string();
                        Some((url, t))
                    })
                    .collect()
            })
            .unwrap_or_default();
        return Ok(base(vec![], true, entries));
    }
    // The chosen format(s): `requested_formats` for a video+audio pair, else the top-level format.
    let fmts: Vec<RawFormat> = match v.get("requested_formats").and_then(|r| r.as_array()) {
        Some(a) if !a.is_empty() => a.iter().filter_map(|f| serde_json::from_value(f.clone()).ok()).collect(),
        _ => serde_json::from_value::<RawFormat>(v.clone()).into_iter().collect(),
    };
    let top_headers: HashMap<String, String> = v.get("http_headers").and_then(|h| serde_json::from_value(h.clone()).ok()).unwrap_or_default();
    let parts: Vec<Part> = fmts
        .into_iter()
        .filter_map(|f| {
            let url = f.url.clone()?;
            let proto = f.protocol.clone().unwrap_or_default();
            let direct = matches!(proto.as_str(), "https" | "http") && !url.contains(".m3u8") && !url.contains(".mpd");
            let none = |c: &Option<String>| c.as_deref().is_none_or(|c| c == "none");
            let (has_video, has_audio) = (!none(&f.vcodec), !none(&f.acodec));
            let headers: Vec<(String, String)> = if f.http_headers.is_empty() { top_headers.clone() } else { f.http_headers.clone() }.into_iter().collect();
            let size = f.filesize.or(f.filesize_approx.map(|a| a as u64));
            let label = match (f.height, f.format_note.as_deref()) {
                (Some(h), _) if has_video => format!("{h}p"),
                (_, Some(n)) if !n.is_empty() => n.to_string(),
                _ => String::new(),
            };
            Some(Part { format_id: f.format_id, ext: f.ext, url, headers, size, has_video, has_audio, direct, label })
        })
        .collect();
    Ok(base(parts, false, vec![]))
}

/// The container for two streams stitched together without re-encoding.
pub fn merged_ext(parts: &[Part]) -> &'static str {
    let v = parts.iter().find(|p| p.has_video).map(|p| p.ext.as_str()).unwrap_or("");
    let a = parts.iter().find(|p| !p.has_video && p.has_audio).map(|p| p.ext.as_str()).unwrap_or("");
    match (v, a) {
        ("mp4", "m4a") | ("mp4", "mp4") | ("m4v", "m4a") => "mp4",
        ("webm", "webm") | ("webm", "opus") => "webm",
        _ => "mkv",
    }
}

/// Plain-language reasons from yt-dlp's complaints.
pub fn friendly_error(stderr: &str) -> String {
    let l = stderr.to_lowercase();
    let pick = |needles: &[&str]| needles.iter().any(|n| l.contains(n));
    if pick(&["unsupported url"]) {
        "yt-dlp doesn't recognise a video on that page. If it's a plain file, add it with “Download as a plain file”.".into()
    } else if pick(&["private video", "this video is private"]) {
        "That video is private. It will only work if you're logged in as someone who can see it — send it from your browser with the extension so your login is used.".into()
    } else if pick(&["sign in to confirm", "confirm you’re not a bot", "confirm you're not a bot", "login required", "log in", "sign in"]) {
        "The site wants you to sign in first. Use the IDMX browser extension (it passes your login along), or choose a browser under Settings → Video downloads to borrow its cookies.".into()
    } else if pick(&["geo restricted", "not available in your country", "geo-restricted"]) {
        "That video isn't available in your country.".into()
    } else if pick(&["video unavailable", "this video is unavailable", "has been removed", "no longer available"]) {
        "That video has been removed or is unavailable.".into()
    } else if pick(&["drm"]) {
        "That video is DRM-protected, which can't be downloaded.".into()
    } else if pick(&["http error 403"]) {
        "The site refused the request (403). Try again from the browser extension so your login is included, or update yt-dlp.".into()
    } else if pick(&["http error 429", "too many requests"]) {
        "The site is limiting requests (429). Wait a few minutes and try again.".into()
    } else if pick(&["ffmpeg", "ffprobe"]) && pick(&["not found", "not installed", "requires"]) {
        "That format needs ffmpeg to combine its video and audio, and ffmpeg wasn't found. Install it (Settings → Video downloads) or pick a lower quality.".into()
    } else if pick(&["requested format is not available"]) {
        "That quality isn't available for this video. Try “Best” or another quality — and if sound and picture come separately, make sure ffmpeg is installed (Settings → Video downloads).".into()
    } else if pick(&["unable to download", "urlopen error", "temporary failure in name resolution", "getaddrinfo", "network is unreachable", "timed out"]) {
        "Couldn't reach the site. Check your connection and try again.".into()
    } else if pick(&["age", "confirm your age"]) && pick(&["restricted", "verify", "confirm"]) {
        "That video is age-restricted. It needs a signed-in browser — use the IDMX extension.".into()
    } else {
        let last = stderr.lines().rev().find(|l| l.trim_start().starts_with("ERROR")).unwrap_or_else(|| stderr.lines().last().unwrap_or("unknown error"));
        format!("yt-dlp couldn't get that video: {}", last.trim().trim_start_matches("ERROR:").trim())
    }
}

// ───────────────────────────── running it ─────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct RunOpts {
    pub cookies_from_browser: Option<String>,
    /// A `Cookie:` header to send (from the browser extension).
    pub cookie_header: Option<String>,
    pub headers: Vec<(String, String)>,
    pub proxy: Option<String>,
    pub ffmpeg: Option<PathBuf>,
    /// Bind to this local address (to choose which network yt-dlp uses).
    pub source_address: Option<String>,
    /// A JavaScript runtime for sites that need one to hand out their best formats.
    pub js_runtime: Option<(String, PathBuf)>,
    /// Segments of an HLS/DASH stream to fetch at once.
    pub fragments: u32,
}

fn common_args(o: &RunOpts) -> Vec<String> {
    let mut a: Vec<String> = vec!["--no-warnings".into(), "--no-playlist".into(), "--ignore-config".into()];
    if let Some(b) = o.cookies_from_browser.as_deref().filter(|b| !b.is_empty()) {
        a.push("--cookies-from-browser".into());
        a.push(b.into());
    }
    if let Some(c) = o.cookie_header.as_deref().filter(|c| !c.is_empty()) {
        a.push("--add-header".into());
        a.push(format!("Cookie:{c}"));
    }
    for (k, v) in &o.headers {
        a.push("--add-header".into());
        a.push(format!("{k}:{v}"));
    }
    if let Some(p) = o.proxy.as_deref().filter(|p| !p.is_empty()) {
        a.push("--proxy".into());
        a.push(p.into());
    }
    if let Some(f) = &o.ffmpeg {
        a.push("--ffmpeg-location".into());
        a.push(f.to_string_lossy().into_owned());
    }
    if let Some((name, path)) = &o.js_runtime {
        a.push("--js-runtimes".into());
        a.push(format!("{name}:{}", path.display()));
    }
    if let Some(ip) = o.source_address.as_deref().filter(|p| !p.is_empty()) {
        a.push("--source-address".into());
        a.push(ip.into());
    }
    a
}

/// Ask yt-dlp what `url` is and which stream(s) `selector` would pick. Nothing is downloaded.
pub async fn extract(tool: &Tool, url: &str, selector: &str, o: &RunOpts, cancel: &CancellationToken) -> Result<Extracted> {
    if is_site_home(url) {
        return Err(Error::msg("That's a website's home page, not a video. Open a video, then copy its link."));
    }
    let mut args = common_args(o);
    args.extend(["--socket-timeout".into(), "20".into(), "-J".into(), "-f".into(), selector.to_string(), "--".into(), url.to_string()]);
    let child = Command::new(&tool.path).args(&args).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true).spawn().map_err(|e| Error::msg(format!("Couldn't start yt-dlp: {e}")))?;
    let out = tokio::select! {
        _ = cancel.cancelled() => return Err(Error::msg("Stopped.")),
        r = tokio::time::timeout(Duration::from_secs(90), child.wait_with_output()) => r.map_err(|_| Error::msg("yt-dlp took too long to look at that page. Check your connection and try again."))??,
    };
    if !out.status.success() {
        return Err(Error::msg(friendly_error(&String::from_utf8_lossy(&out.stderr))));
    }
    parse_extracted(&String::from_utf8_lossy(&out.stdout), url)
}

/// What yt-dlp's own downloader reports.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    pub downloaded: u64,
    pub total: Option<u64>,
    pub speed: f64,
    pub eta: Option<u64>,
    pub finished: bool,
}

/// `IDMX|downloaded|total|estimate|speed|eta|status` as produced by our `--progress-template`.
pub fn parse_progress(line: &str) -> Option<Progress> {
    let rest = line.trim().strip_prefix("IDMX|")?;
    let f: Vec<&str> = rest.split('|').collect();
    if f.len() < 6 {
        return None;
    }
    let num = |s: &str| s.trim().parse::<f64>().ok();
    Some(Progress {
        downloaded: num(f[0]).unwrap_or(0.0) as u64,
        total: num(f[1]).or_else(|| num(f[2])).map(|t| t as u64).filter(|t| *t > 0),
        speed: num(f[3]).unwrap_or(0.0),
        eta: num(f[4]).map(|e| e as u64),
        finished: f[5].trim() == "finished",
    })
}

/// Let yt-dlp download it. Calls `on_progress` as it goes; resolves with the file it wrote.
#[allow(clippy::too_many_arguments)]
pub async fn download_with_ytdlp(tool: &Tool, url: &str, selector: &str, home: &Path, temp: &Path, o: &RunOpts, limit: Option<u64>, cancel: &CancellationToken, on_progress: impl Fn(Progress)) -> Result<PathBuf> {
    let mut args = common_args(o);
    args.extend([
        "-f".into(),
        selector.to_string(),
        "--newline".into(),
        "--concurrent-fragments".into(),
        o.fragments.max(1).to_string(),
        "--progress-template".into(),
        "download:IDMX|%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s|%(progress.eta)s|%(progress.status)s".into(),
        "--print".into(),
        "after_move:IDMXFILE|%(filepath)s".into(),
        "-P".into(),
        format!("home:{}", home.display()),
        "-P".into(),
        format!("temp:{}", temp.display()),
        "-o".into(),
        "%(title).150B [%(id)s].%(ext)s".into(),
        "--no-mtime".into(),
    ]);
    if let Some(l) = limit {
        args.push("--limit-rate".into());
        args.push(l.to_string());
    }
    args.extend(["--".into(), url.to_string()]);
    let mut child = Command::new(&tool.path).args(&args).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true).spawn().map_err(|e| Error::msg(format!("Couldn't start yt-dlp: {e}")))?;
    let stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let err_task = tokio::spawn(async move {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s).await;
        s
    });
    let mut lines = BufReader::new(stdout).lines();
    let mut file: Option<PathBuf> = None;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                let _ = child.kill().await;
                return Err(Error::msg("Stopped."));
            }
            line = lines.next_line() => match line {
                Ok(Some(l)) => {
                    if let Some(p) = parse_progress(&l) { on_progress(p); }
                    else if let Some(f) = l.strip_prefix("IDMXFILE|") { file = Some(PathBuf::from(f.trim())); }
                }
                _ => break,
            }
        }
    }
    let status = child.wait().await?;
    let stderr = err_task.await.unwrap_or_default();
    if !status.success() {
        return Err(Error::msg(friendly_error(&stderr)));
    }
    file.filter(|p| p.exists()).ok_or_else(|| Error::msg("yt-dlp finished but didn't say where it saved the file."))
}

/// Combine a video stream and an audio stream into one file without re-encoding.
pub async fn merge(ffmpeg: &Path, video: &Path, audio: &Path, out: &Path) -> Result<()> {
    let output = Command::new(ffmpeg)
        .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(video)
        .arg("-i")
        .arg(audio)
        .args(["-c", "copy", "-map", "0:v:0", "-map", "1:a:0"])
        .arg(out)
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| Error::msg(format!("Couldn't start ffmpeg: {e}")))?;
    if !output.status.success() {
        let e = String::from_utf8_lossy(&output.stderr);
        return Err(Error::msg(format!("ffmpeg couldn't combine the video and audio: {}", e.lines().last().unwrap_or("unknown error"))));
    }
    Ok(())
}

/// `https://www.youtube.com/` and the like: yt-dlp would try to read a whole recommendations feed.
pub fn is_site_home(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| matches!(u.path(), "" | "/") && u.query().is_none_or(|q| !q.contains("v=") && !q.contains("list=")))
}

/// A rough check that a URL is a page worth sending to yt-dlp without asking the server first.
pub fn looks_like_media_page(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else { return false };
    let host = u.host_str().unwrap_or("").trim_start_matches("www.").trim_start_matches("m.").to_ascii_lowercase();
    let path = u.path();
    const SITES: &[&str] = &[
        "youtube.com", "youtu.be", "youtube-nocookie.com", "vimeo.com", "twitch.tv", "dailymotion.com", "tiktok.com", "twitter.com", "x.com",
        "facebook.com", "fb.watch", "instagram.com", "reddit.com", "soundcloud.com", "bandcamp.com", "bilibili.com", "nicovideo.jp", "rumble.com",
        "odysee.com", "streamable.com", "imgur.com", "vk.com", "ok.ru", "mixcloud.com", "archive.org", "ted.com", "arte.tv", "bbc.co.uk", "ndr.de", "zdf.de",
    ];
    let on_site = SITES.iter().any(|s| host == *s || host.ends_with(&format!(".{s}")));
    if !on_site {
        return false;
    }
    // A site's home page or a plain image isn't a video.
    path.len() > 1 && !path.rsplit('/').next().is_some_and(|f| f.ends_with(".jpg") || f.ends_with(".png"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_pages_are_not_videos() {
        assert!(is_site_home("https://www.youtube.com/"));
        assert!(is_site_home("https://youtube.com"));
        assert!(!is_site_home("https://www.youtube.com/watch?v=abc"));
        assert!(!is_site_home("https://youtu.be/abc"));
    }

    const PAIR: &str = r#"{"id":"abc","title":"My Video","uploader":"Chan","duration":212.4,"thumbnail":"https://i/x.jpg","extractor_key":"Youtube","webpage_url":"https://www.youtube.com/watch?v=abc",
      "requested_formats":[
        {"format_id":"137","ext":"mp4","url":"https://rr.googlevideo.com/v","protocol":"https","height":1080,"vcodec":"avc1","acodec":"none","filesize":50000000,"http_headers":{"User-Agent":"UA","Accept":"*/*"}},
        {"format_id":"140","ext":"m4a","url":"https://rr.googlevideo.com/a","protocol":"https","vcodec":"none","acodec":"mp4a","filesize_approx":3000000.0}]}"#;

    #[test]
    fn video_plus_audio_is_two_direct_parts_merged_to_mp4() {
        let e = parse_extracted(PAIR, "https://youtu.be/abc").unwrap();
        assert_eq!((e.title.as_str(), e.duration, e.parts.len()), ("My Video", Some(212), 2));
        assert!(e.all_direct() && e.needs_merge());
        assert!(e.parts[0].has_video && !e.parts[0].has_audio);
        assert!(!e.parts[1].has_video && e.parts[1].has_audio);
        assert_eq!(e.parts[0].size, Some(50_000_000));
        assert_eq!(e.parts[1].size, Some(3_000_000));
        assert_eq!(merged_ext(&e.parts), "mp4");
        assert_eq!(e.label(), "1080p · mp4");
        assert!(e.parts[0].headers.iter().any(|(k, v)| k == "User-Agent" && v == "UA"));
    }

    #[test]
    fn a_single_progressive_file_uses_the_top_level_format() {
        let j = r#"{"id":"1","title":"Clip","ext":"mp4","url":"https://cdn/x.mp4","protocol":"https","height":720,"vcodec":"avc1","acodec":"mp4a","http_headers":{"Referer":"https://site/"}}"#;
        let e = parse_extracted(j, "https://site/v/1").unwrap();
        assert_eq!(e.parts.len(), 1);
        assert!(e.all_direct() && !e.needs_merge());
        assert!(e.parts[0].has_video && e.parts[0].has_audio);
        assert_eq!(e.parts[0].headers, vec![("Referer".to_string(), "https://site/".to_string())]);
        assert_eq!(e.label(), "720p · mp4");
    }

    #[test]
    fn streaming_formats_are_not_direct() {
        let hls = r#"{"id":"1","title":"Live","ext":"mp4","url":"https://cdn/master.m3u8","protocol":"m3u8_native","vcodec":"avc1","acodec":"mp4a"}"#;
        assert!(!parse_extracted(hls, "u").unwrap().all_direct());
        let dash = r#"{"id":"1","title":"D","ext":"mp4","url":"https://cdn/m.mpd","protocol":"http_dash_segments","vcodec":"avc1","acodec":"none"}"#;
        assert!(!parse_extracted(dash, "u").unwrap().all_direct());
        // https protocol but an .m3u8 URL is still a playlist.
        let sneaky = r#"{"id":"1","title":"S","ext":"mp4","url":"https://cdn/a.m3u8?x=1","protocol":"https","vcodec":"avc1","acodec":"mp4a"}"#;
        assert!(!parse_extracted(sneaky, "u").unwrap().all_direct());
    }

    #[test]
    fn audio_only_and_playlists() {
        let a = r#"{"id":"1","title":"Song","ext":"m4a","url":"https://cdn/a.m4a","protocol":"https","vcodec":"none","acodec":"mp4a"}"#;
        let e = parse_extracted(a, "u").unwrap();
        assert!(!e.parts[0].has_video && e.parts[0].has_audio);
        assert_eq!(e.label(), "audio · m4a");
        let p = r#"{"_type":"playlist","id":"pl","title":"List","entries":[{"title":"One","url":"https://x/1"},{"title":"Two","webpage_url":"https://x/2"}]}"#;
        let e = parse_extracted(p, "u").unwrap();
        assert!(e.is_playlist && e.parts.is_empty());
        assert_eq!(e.playlist_entries, vec![("https://x/1".to_string(), "One".to_string()), ("https://x/2".to_string(), "Two".to_string())]);
        assert!(parse_extracted("not json", "u").is_err());
    }

    #[test]
    fn container_choice_never_forces_a_re_encode() {
        let p = |ext: &str, v: bool| Part { format_id: "x".into(), ext: ext.into(), url: String::new(), headers: vec![], size: None, has_video: v, has_audio: !v, direct: true, label: String::new() };
        assert_eq!(merged_ext(&[p("mp4", true), p("m4a", false)]), "mp4");
        assert_eq!(merged_ext(&[p("webm", true), p("webm", false)]), "webm");
        assert_eq!(merged_ext(&[p("webm", true), p("m4a", false)]), "mkv");
    }

    #[test]
    fn quality_presets() {
        assert_eq!(selector_for("best", true), "bv*+ba/b");
        assert_eq!(selector_for("", true), "bv*+ba/b");
        assert_eq!(selector_for("best", false), "b");
        assert_eq!(selector_for("720", true), "bv*[height<=720]+ba/b[height<=720]/b");
        assert_eq!(selector_for("720", false), "b[height<=720]/b");
        assert!(selector_for("audio", true).starts_with("ba"));
        assert!(selector_for("audio320", true).starts_with("ba"));
        assert_eq!(audio_bitrate("audio216"), Some(216));
        assert_eq!(audio_bitrate("audio"), None);
        assert_eq!(audio_bitrate("720"), None);
        assert_eq!(selector_for("bestvideo[height<=?1080]", true), "bestvideo[height<=?1080]", "a custom selector passes through");
    }

    #[test]
    fn progress_lines() {
        let p = parse_progress("IDMX|1048576|52428800|NA|2097152.5|24|downloading").unwrap();
        assert_eq!((p.downloaded, p.total, p.speed, p.eta, p.finished), (1_048_576, Some(52_428_800), 2_097_152.5, Some(24), false));
        let p = parse_progress("IDMX|500|NA|1000|NA|NA|finished").unwrap();
        assert_eq!((p.total, p.speed, p.eta, p.finished), (Some(1000), 0.0, None, true));
        assert!(parse_progress("[download] 10%").is_none());
        assert!(parse_progress("IDMX|only|three").is_none());
    }

    #[test]
    fn yt_dlp_complaints_become_plain_sentences() {
        assert!(friendly_error("ERROR: Unsupported URL: https://x").contains("doesn't recognise"));
        assert!(friendly_error("ERROR: [youtube] abc: Private video. Sign in").contains("private"));
        assert!(friendly_error("ERROR: [youtube] abc: Sign in to confirm you’re not a bot").contains("sign in"));
        assert!(friendly_error("ERROR: Video unavailable").contains("removed"));
        assert!(friendly_error("ERROR: unable to download video data: HTTP Error 403: Forbidden").contains("403"));
        assert!(friendly_error("ERROR: Requested format is not available. ffmpeg is not installed").contains("ffmpeg"));
        let other = friendly_error("some noise\nERROR: Something very strange happened");
        assert!(other.contains("Something very strange happened"), "{other}");
    }

    #[test]
    fn checksum_file_parsing_and_asset_names() {
        let sums = "aaaa1111  yt-dlp\nBBBB2222 *yt-dlp_macos\ncccc3333  yt-dlp.exe\n";
        assert_eq!(expected_sha(sums, "yt-dlp_macos").as_deref(), Some("bbbb2222"));
        assert_eq!(expected_sha(sums, "yt-dlp").as_deref(), Some("aaaa1111"));
        assert!(expected_sha(sums, "nope").is_none());
        assert!(ytdlp_asset().is_ok(), "this machine has a known yt-dlp build");
    }

    #[test]
    fn which_pages_are_sent_to_yt_dlp() {
        for yes in ["https://www.youtube.com/watch?v=abc", "https://youtu.be/abc", "https://vimeo.com/123", "https://m.youtube.com/watch?v=1", "https://x.com/u/status/1", "https://www.tiktok.com/@u/video/1"] {
            assert!(looks_like_media_page(yes), "{yes}");
        }
        for no in ["https://example.com/file.zip", "https://youtube.com/", "https://notyoutube.com/watch", "not a url", "https://i.imgur.com/a.png"] {
            assert!(!looks_like_media_page(no), "{no}");
        }
    }

    #[test]
    fn single_checksum_files_and_runtime_choice() {
        let h = "a".repeat(64);
        assert_eq!(expected_sha_single(&format!("{h}  deno.zip\n"), "x").as_deref(), Some(h.as_str()));
        assert_eq!(expected_sha_single(&format!("{}\n", h.to_uppercase()), "x").as_deref(), Some(h.as_str()));
        assert!(expected_sha_single("not a hash", "x").is_none());
        assert!(deno_asset().is_ok() && ffmpeg_asset().is_ok());
        assert_eq!(ffmpeg_asset().unwrap().1.len(), 64);
        let d = tempfile::tempdir().unwrap();
        let node = d.path().join("node");
        std::fs::write(&node, b"x").unwrap();
        assert_eq!(find_js_runtime(node.to_str().unwrap(), d.path()).unwrap().0, "node");
        let o = RunOpts { js_runtime: Some(("deno".into(), "/x/deno".into())), ..Default::default() };
        let a = common_args(&o);
        let i = a.iter().position(|x| x == "--js-runtimes").unwrap();
        assert_eq!(a[i + 1], "deno:/x/deno");
        assert!(friendly_error("ERROR: Requested format is not available").contains("ffmpeg"));
    }

    #[test]
    fn finds_tools_in_the_managed_folder_first() {
        let d = tempfile::tempdir().unwrap();
        assert!(find_tool("definitely-not-installed-xyz", "", d.path()).is_none());
        std::fs::create_dir_all(bin_dir(d.path())).unwrap();
        std::fs::write(bin_dir(d.path()).join(exe("faketool")), b"x").unwrap();
        let (p, managed) = find_tool("faketool", "", d.path()).unwrap();
        assert!(managed && p.ends_with(exe("faketool")));
        // An explicit path wins, and is not "managed".
        let own = d.path().join("mine");
        std::fs::write(&own, b"x").unwrap();
        let (p, managed) = find_tool("faketool", own.to_str().unwrap(), d.path()).unwrap();
        assert!(!managed && p == own);
        assert!(find_tool("faketool", "/nope/missing", d.path()).is_none());
    }
}
