//! The public data model. Everything here crosses the API boundary, so it is all serde/camelCase.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ───────────────────────────── networks ─────────────────────────────

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum IfKind {
    Wifi,
    Ethernet,
    Cellular,
    Tether,
    Vpn,
    Bridge,
    Virtual,
    Loopback,
    Unknown,
}

impl IfKind {
    /// Kinds that are worth downloading over unless the user says otherwise.
    pub fn default_enabled(self) -> bool {
        matches!(
            self,
            IfKind::Wifi | IfKind::Ethernet | IfKind::Cellular | IfKind::Tether | IfKind::Unknown
        )
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NetInterface {
    /// OS device name — `en0`, `eth1`, `Wi-Fi`. Stable id used everywhere.
    pub id: String,
    /// Human label — "Wi-Fi", "iPhone USB".
    pub label: String,
    pub kind: IfKind,
    pub ipv4: Vec<String>,
    pub ipv6: Vec<String>,
    pub gateway: Option<String>,
    /// IPv4 networks this interface sits on, as `10.0.0.0/24`.
    #[serde(default)]
    pub subnets: Vec<String>,
    pub mac: Option<String>,
    pub up: bool,
    /// Link speed as reported by the OS, megabits/s.
    pub link_mbps: Option<u64>,
    /// Has an address we can bind a socket to.
    pub usable: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CapPeriod {
    Daily,
    Monthly,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct InterfaceRule {
    pub enabled: Option<bool>,
    pub label: Option<String>,
    pub color: Option<String>,
    /// Bytes/second ceiling for this network across every download.
    pub speed_limit: Option<u64>,
    /// A metered link (cellular hotspot): only joins big downloads and honours the cap.
    pub metered: bool,
    pub data_cap: Option<u64>,
    pub cap_period: CapPeriod,
    /// Only take part in files at least this large.
    pub min_file_size: Option<u64>,
    pub streams: Option<u32>,
}

impl Default for InterfaceRule {
    fn default() -> Self {
        Self {
            enabled: None,
            label: None,
            color: None,
            speed_limit: None,
            metered: false,
            data_cap: None,
            cap_period: CapPeriod::Monthly,
            min_file_size: None,
            streams: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NetState {
    /// Up, and not carrying anything right now.
    Idle,
    Active,
    /// Connected, but the server can't be reached through it. One connection keeps trying and
    /// the rest follow once it gets through.
    Unreachable,
    /// Reachable, but the server kept refusing requests over it (see the error).
    Failed,
    /// Data cap reached.
    Capped,
    /// Not connected to this computer. Used again as soon as it is.
    Offline,
    /// Switched off by the user.
    Disabled,
}

/// A network as the UI shows it: hardware facts + the user's rule + live health.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InterfaceInfo {
    #[serde(flatten)]
    pub iface: NetInterface,
    pub rule: InterfaceRule,
    pub enabled: bool,
    /// 0–100, from latency, error rate and stalls over the last minute.
    pub health: u8,
    pub latency_ms: Option<f64>,
    pub used_bytes: u64,
    pub speed: f64,
    pub shares_gateway_with: Vec<String>,
    /// Other usable networks on the same IPv4 subnet: they share one link, so they won't add up.
    pub shares_subnet_with: Vec<String>,
    /// Whether the last ping got through. `None` until the first one.
    pub reachable: Option<bool>,
    pub state: NetState,
}

// ───────────────────────────── jobs ─────────────────────────────

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum JobStatus {
    Queued,
    Probing,
    Downloading,
    Paused,
    Verifying,
    Processing,
    /// A torrent that has everything and is now sharing it back. Not a download slot.
    Seeding,
    Completed,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn is_running(self) -> bool {
        matches!(
            self,
            JobStatus::Probing | JobStatus::Downloading | JobStatus::Verifying | JobStatus::Processing
        )
    }
    pub fn is_finished(self) -> bool {
        matches!(self, JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BasicAuth {
    pub user: String,
    pub password: String,
}

/// What a job downloads.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum JobKind {
    /// A file at a URL, split across your networks.
    #[default]
    Http,
    /// A magnet link or `.torrent`.
    Torrent,
    /// A video or audio page, resolved with yt-dlp.
    Media,
}

/// A file inside a torrent.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TorrentFile {
    pub index: u32,
    pub path: String,
    pub size: u64,
    pub downloaded: u64,
    pub selected: bool,
}

/// Live state of a torrent download.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct TorrentStatus {
    pub info_hash: String,
    /// Peers we're connected to right now.
    pub peers: u32,
    /// Peers we know of that we aren't connected to.
    pub known_peers: u32,
    pub uploaded: u64,
    pub upload_speed: f64,
    /// Uploaded ÷ downloaded.
    pub ratio: f64,
    /// `initializing` · `downloading` · `seeding` · `paused` · `error`
    pub state: String,
    pub files: Vec<TorrentFile>,
    /// Waiting for metadata from peers (a magnet link).
    pub fetching_metadata: bool,
}

/// What a media job found and chose.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct MediaStatus {
    pub title: String,
    pub uploader: Option<String>,
    pub thumbnail: Option<String>,
    pub duration: Option<u64>,
    pub extractor: Option<String>,
    /// What was chosen, e.g. "1080p · mp4".
    pub format: String,
    /// `direct` (IDMX's multi-network engine) or `ytdlp` (yt-dlp's own downloader).
    pub strategy: String,
    pub parts: u32,
}

/// Per-download knobs. Anything left `None` falls back to the global settings.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct JobOptions {
    /// Networks to use. `None` = every enabled network.
    pub interfaces: Option<Vec<String>>,
    pub streams_per_network: Option<u32>,
    pub speed_limit: Option<u64>,
    pub headers: Vec<(String, String)>,
    /// Raw `Cookie:` header value.
    pub cookies: Option<String>,
    pub auth: Option<BasicAuth>,
    pub bearer: Option<String>,
    pub referer: Option<String>,
    pub user_agent: Option<String>,
    pub proxy: Option<String>,
    /// `sha256:<hex>`, bare hex, or `auto` to look for a published checksum.
    pub checksum: Option<String>,
    pub file_name: Option<String>,
    /// Only run inside the global schedule window.
    pub scheduled_window: bool,
    /// Skip post-download rules.
    pub skip_post: bool,
    /// Torrent: which files to fetch (indexes). `None` = all.
    pub torrent_files: Option<Vec<u32>>,
    /// Treat the link as a page with video on it and resolve it with yt-dlp.
    pub media: Option<bool>,
    /// Media: a yt-dlp format selector, or a preset (`best`, `1080`, `720`, `480`, `audio`).
    pub media_format: Option<String>,
    /// Never send this to yt-dlp, even if the page looks like a video page.
    pub force_http: bool,
    /// If the file changes on the server mid-way, start over instead of stopping.
    pub restart_if_changed: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NetStat {
    pub id: String,
    pub label: String,
    pub kind: IfKind,
    pub bytes: u64,
    pub speed: f64,
    pub streams: u32,
    pub active: u32,
    pub latency_ms: f64,
    pub health: u8,
    pub errors: u32,
    pub state: NetState,
    /// What went wrong most recently on this network, while it is not healthy.
    #[serde(default)]
    pub error: Option<String>,
    /// Whether the user has it switched on for this download.
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// One connection to the server, through one network.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StreamStat {
    pub id: u32,
    pub net: String,
    /// `idle` · `downloading` · `racing` · `retrying` · `parked`
    pub state: String,
    /// 1-based chunk number being fetched.
    pub block: Option<u32>,
    pub bytes: u64,
    pub speed: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ChecksumResult {
    pub algo: String,
    pub actual: Option<String>,
    pub expected: Option<String>,
    /// `Some(true)` verified, `Some(false)` mismatch, `None` nothing to compare against.
    pub verified: Option<bool>,
    /// Where the expected value came from: `user`, `header`, `sidecar`.
    pub source: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub url: String,
    pub final_url: Option<String>,
    pub file_name: String,
    pub dest_dir: String,
    /// Final path once completed.
    pub path: Option<String>,
    pub status: JobStatus,
    pub size: Option<u64>,
    pub downloaded: u64,
    pub speed: f64,
    pub eta: Option<u64>,
    pub error: Option<String>,
    pub resumable: bool,
    pub priority: i32,
    pub start_at: Option<i64>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub options: JobOptions,
    pub networks: Vec<NetStat>,
    pub blocks_total: u32,
    pub blocks_done: u32,
    pub checksum: Option<ChecksumResult>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// Free-form lines from verification and post-processing.
    pub log: Vec<String>,
    pub mime: Option<String>,
    #[serde(default)]
    pub kind: JobKind,
    #[serde(default)]
    pub torrent: Option<TorrentStatus>,
    #[serde(default)]
    pub media: Option<MediaStatus>,
    /// A part of another job (one stream of a video, say). Hidden from the list.
    #[serde(default)]
    pub parent: Option<String>,
    /// The most connections it has had open at once.
    #[serde(default)]
    pub peak_streams: u32,
    /// Overall speed (bytes/s) sampled every couple of seconds, kept for the finished summary.
    #[serde(default)]
    pub speed_history: Vec<f32>,
}

/// Per-block state for the progress map. `state`: 0 pending · 1 downloading · 2 done.
/// `owner`: index into `networks` of the interface that wrote the block (255 = none).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BlockMap {
    pub block_size: u64,
    pub networks: Vec<String>,
    pub state: Vec<u8>,
    pub owner: Vec<u8>,
    /// Fraction 0..1 of the block that is on disk, for the half-filled look.
    pub fill: Vec<f32>,
    /// Blocks several networks each delivered part of: `(block index, [(network index, percent)])`.
    #[serde(default)]
    pub split: Vec<(u32, Vec<(u8, u8)>)>,
}

// ───────────────────────────── settings ─────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct RuleMatch {
    /// Lower-case extensions without the dot.
    pub extensions: Vec<String>,
    pub name_regex: Option<String>,
    pub host: Option<String>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum PostAction {
    /// Move to a directory. Supports `{ext}` `{host}` `{year}` `{month}` `{date}` `{name}`.
    MoveTo { dir: String },
    /// Unpack zip / tar / tar.gz next to the file (or into `dir`).
    Extract { dir: Option<String>, delete_archive: bool },
    /// Run a shell command with `IDMX_FILE`, `IDMX_URL`, `IDMX_NAME`, `IDMX_SIZE`, `IDMX_SHA256` set.
    Run { command: String },
    Reveal,
    Notify { message: Option<String> },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PostRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub when: RuleMatch,
    pub actions: Vec<PostAction>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ScheduleWindow {
    pub enabled: bool,
    /// Minutes after local midnight.
    pub start_minute: u32,
    pub end_minute: u32,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum QueueEmpty {
    #[default]
    Nothing,
    Sleep,
    Quit,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct RemoteSettings {
    pub enabled: bool,
    pub port: u16,
    /// Listen on every interface instead of loopback only.
    pub lan: bool,
    pub token: String,
}

impl Default for RemoteSettings {
    fn default() -> Self {
        Self { enabled: false, port: 7733, lan: false, token: String::new() }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub download_dir: String,
    pub max_concurrent_jobs: u32,
    pub default_streams: u32,
    pub max_streams: u32,
    pub global_speed_limit: Option<u64>,
    pub interfaces: BTreeMap<String, InterfaceRule>,
    pub proxy: Option<String>,
    pub user_agent: Option<String>,
    pub schedule: ScheduleWindow,
    pub post_rules: Vec<PostRule>,
    pub on_queue_empty: QueueEmpty,
    pub remote: RemoteSettings,
    pub notifications: bool,
    /// Race slow blocks at the tail of a download.
    pub hedging: bool,
    /// Compute SHA-256 of finished downloads (up to `hash_limit` bytes).
    pub hash_on_complete: bool,
    pub hash_limit: u64,
    /// Watch the clipboard in the desktop app for download links.
    pub clipboard_watch: bool,
    /// `owner/name` of the GitHub repository to look for new releases in. Empty = never check.
    pub update_repo: String,
    /// The release the user dismissed the banner for.
    pub dismissed_update: Option<String>,
    /// If a file changed on the server since a download began, stop instead of starting over.
    pub refuse_changed_resume: bool,
    pub torrent: TorrentSettings,
    pub media: MediaSettings,
    /// The welcome flow has been shown.
    pub onboarded: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct TorrentSettings {
    pub enabled: bool,
    /// Network to bind torrent traffic to. `None` = the system's default route.
    pub network: Option<String>,
    /// Use every enabled network at once, giving each its own share of a multi-file torrent.
    pub multi_network: bool,
    /// Keep sharing after the download is complete.
    pub seed: bool,
    /// Stop seeding at this upload ratio (0 = no ratio limit).
    pub seed_ratio: f64,
    /// Stop seeding after this many minutes (0 = no time limit).
    pub seed_minutes: u32,
    pub dht: bool,
    pub listen_port: Option<u16>,
    pub upload_limit: Option<u64>,
    pub peer_limit: Option<u32>,
}

impl Default for TorrentSettings {
    fn default() -> Self {
        Self { enabled: true, network: None, multi_network: true, seed: true, seed_ratio: 1.0, seed_minutes: 0, dht: true, listen_port: None, upload_limit: None, peer_limit: None }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct MediaSettings {
    /// Send video pages to yt-dlp automatically.
    pub enabled: bool,
    /// Where yt-dlp is, if you installed it yourself. Empty = the one IDMX manages (or the one on PATH).
    pub ytdlp_path: String,
    pub ffmpeg_path: String,
    /// A JavaScript runtime (deno, node, bun). Empty = the one IDMX manages, or one on your PATH.
    pub js_runtime_path: String,
    /// Default quality: `best`, `1080`, `720`, `480`, `audio`.
    pub default_quality: String,
    /// Fetch the video's subtitles, thumbnail and metadata too.
    pub embed_extras: bool,
    /// Pass this browser's cookies to yt-dlp (`chrome`, `firefox`, `safari`, …). Empty = none.
    pub cookies_from_browser: String,
}

impl Default for MediaSettings {
    fn default() -> Self {
        Self { enabled: true, ytdlp_path: String::new(), ffmpeg_path: String::new(), js_runtime_path: String::new(), default_quality: "best".into(), embed_extras: false, cookies_from_browser: String::new() }
    }
}

impl Default for Settings {
    fn default() -> Self {
        let dir = directories::UserDirs::new()
            .and_then(|d| d.download_dir().map(|p| p.to_string_lossy().into_owned()))
            .unwrap_or_else(|| ".".into());
        Self {
            download_dir: dir,
            max_concurrent_jobs: 3,
            default_streams: 8,
            max_streams: 32,
            global_speed_limit: None,
            interfaces: BTreeMap::new(),
            proxy: None,
            user_agent: None,
            schedule: ScheduleWindow { enabled: false, start_minute: 60, end_minute: 360 },
            post_rules: Vec::new(),
            on_queue_empty: QueueEmpty::Nothing,
            remote: RemoteSettings::default(),
            notifications: true,
            hedging: true,
            hash_on_complete: true,
            hash_limit: 4 << 30,
            clipboard_watch: false,
            update_repo: String::new(),
            dismissed_update: None,
            refuse_changed_resume: true,
            torrent: TorrentSettings::default(),
            media: MediaSettings::default(),
            onboarded: false,
        }
    }
}

// ───────────────────────────── requests & events ─────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct AddRequest {
    /// URLs, one per entry. Each may also contain a batch pattern like `file[01-20].zip`.
    pub urls: Vec<String>,
    pub dest_dir: Option<String>,
    pub options: JobOptions,
    pub paused: bool,
    pub priority: i32,
    pub start_at: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProbeInfo {
    pub url: String,
    pub final_url: String,
    pub file_name: String,
    pub size: Option<u64>,
    pub ranges: bool,
    pub mime: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub status: u16,
    pub ttfb_ms: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct JobTick {
    pub id: String,
    pub status: Option<JobStatus>,
    pub downloaded: u64,
    pub speed: f64,
    pub eta: Option<u64>,
    pub blocks_done: u32,
    pub networks: Vec<NetStat>,
    #[serde(default)]
    pub torrent: Option<TorrentStatus>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct GlobalStats {
    pub speed: f64,
    pub active: u32,
    pub queued: u32,
    pub completed: u32,
    pub failed: u32,
    /// Torrents currently sharing.
    pub seeding: u32,
    pub total_bytes: u64,
    pub session_bytes: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SpeedTestNet {
    pub id: String,
    pub label: String,
    pub kind: IfKind,
    pub mbps: f64,
    pub latency_ms: Option<f64>,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SpeedTestReport {
    pub phase: String,
    pub done: bool,
    pub nets: Vec<SpeedTestNet>,
    pub combined_mbps: Option<f64>,
    pub sum_mbps: f64,
    /// combined / best single network.
    pub gain: Option<f64>,
    pub warnings: Vec<String>,
}

/// One network's numbers over a whole in-depth speed test run.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct EnduranceNet {
    pub id: String,
    pub label: String,
    pub kind: Option<IfKind>,
    /// Throughput over the last second.
    pub mbps: f64,
    pub avg_mbps: f64,
    pub min_mbps: f64,
    pub max_mbps: f64,
    /// 5th percentile: the speed you get on a bad second.
    pub low_mbps: f64,
    pub rtt_ms: Option<f64>,
    pub min_rtt_ms: Option<f64>,
    pub avg_rtt_ms: Option<f64>,
    pub p95_rtt_ms: Option<f64>,
    pub max_rtt_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
    /// Pings sent / not answered.
    pub sent: u64,
    pub lost: u64,
    pub loss_pct: f64,
    /// Most pings in a row that went unanswered.
    pub longest_loss_run: u32,
    /// Seconds with next to no data flowing.
    pub stall_secs: u64,
    pub errors: u64,
    pub bytes: u64,
    pub cycles: u32,
    pub cycle_pct: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct EndurancePoint {
    /// Seconds since the run started.
    pub t: u64,
    pub mbps: f64,
    pub rtt_ms: Option<f64>,
    pub lost: bool,
}

/// What a finished in-depth run left behind (also written next to the log as JSON).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct EnduranceSummary {
    pub name: String,
    pub started_at: i64,
    pub ended_at: i64,
    pub duration_secs: u64,
    pub url: String,
    pub total_bytes: u64,
    pub nets: Vec<EnduranceNet>,
    pub notes: Vec<String>,
    pub csv_path: String,
    pub csv_bytes: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct EnduranceStatus {
    pub running: bool,
    pub phase: String,
    pub started_at: i64,
    pub elapsed_secs: u64,
    pub cycle_bytes: u64,
    pub total_bytes: u64,
    pub log_bytes: u64,
    pub nets: Vec<EnduranceNet>,
    /// The last couple of minutes, for the live chart. The full history only goes to the log file.
    pub recent: Vec<EndurancePoint>,
    pub summary: Option<EnduranceSummary>,
    pub warnings: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    /// The release page to open.
    pub url: String,
    pub dismissed: bool,
}

/// A helper program IDMX can use.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub path: String,
    pub version: String,
    /// Installed by IDMX itself.
    pub managed: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct InstallProgress {
    pub tool: String,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToolsStatus {
    pub ytdlp: Option<ToolInfo>,
    pub ffmpeg: Option<ToolInfo>,
    /// The JavaScript runtime yt-dlp will use (`version` says which: "deno 2.1.4").
    pub js: Option<ToolInfo>,
    pub installing: Option<InstallProgress>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TorrentMetaFile {
    pub index: u32,
    pub path: String,
    pub size: u64,
}

/// What is inside a torrent, for the Add sheet.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TorrentMeta {
    pub name: String,
    pub info_hash: String,
    pub total: u64,
    pub files: Vec<TorrentMetaFile>,
}

/// The browser extension, as last heard from.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionInfo {
    pub version: String,
    pub browser: String,
    pub seen_at: i64,
}

/// What a video page offers, for the Add sheet.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MediaProbe {
    pub title: String,
    pub uploader: Option<String>,
    pub thumbnail: Option<String>,
    pub duration: Option<u64>,
    pub extractor: Option<String>,
    pub heights: Vec<u32>,
    pub format: String,
    pub size: Option<u64>,
    pub direct: bool,
    pub playlist: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", tag = "type", content = "data")]
pub enum Event {
    Tick { stats: GlobalStats, jobs: Vec<JobTick> },
    JobAdded(Box<Job>),
    JobUpdated(Box<Job>),
    JobRemoved(String),
    Interfaces(Vec<InterfaceInfo>),
    Settings(Box<Settings>),
    SpeedTest(SpeedTestReport),
    Endurance(Box<EnduranceStatus>),
    Update(Option<UpdateInfo>),
    Tools(ToolsStatus),
    Extension(Option<ExtensionInfo>),
    Notice { level: String, message: String },
    /// Something only the desktop shell can do.
    Action { action: String, path: Option<String>, job_id: Option<String> },
}
