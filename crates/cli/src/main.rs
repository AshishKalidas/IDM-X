mod fmt;
mod remote;
mod view;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use idmx_core::types::*;
use idmx_core::{Engine, EngineOptions};
use remote::Remote;
use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use view::{frame, Painter, View, BOLD, DIM, RESET};

#[derive(Parser)]
#[command(name = "idmx", version, about = "Download faster by using every network you have at once.", long_about = None)]
struct Cli {
    /// Where IDMX keeps its state.
    #[arg(long, global = true, env = "IDMX_HOME", value_name = "DIR")]
    home: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Download one or more URLs and watch them arrive.
    Get(Fetch),
    /// Queue downloads in a running IDMX and return immediately.
    Add(Fetch),
    /// List downloads.
    #[command(alias = "list")]
    Ls {
        #[arg(long)]
        json: bool,
    },
    /// One-line summary of what is happening.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Pause a download (or `all`).
    Pause { id: String },
    /// Resume a download (or `all`).
    Resume { id: String },
    /// Cancel a download and delete what was fetched so far.
    Cancel { id: String },
    /// Remove a download from the list.
    Rm {
        id: String,
        /// Also delete the finished file.
        #[arg(long)]
        delete_file: bool,
    },
    /// Move a download to the front of the queue.
    Top { id: String },
    /// Show the networks IDMX can use, with a live latency check on each.
    Nets {
        #[arg(long)]
        json: bool,
        /// Include interfaces that can't carry downloads (down, virtual, no address).
        #[arg(long)]
        all: bool,
        /// Skip the latency check.
        #[arg(long)]
        no_ping: bool,
    },
    /// Switch one network on or off for one download, while it runs.
    Net {
        /// Download id (or a unique start of it).
        job: String,
        /// Network id, from `idmx nets`.
        network: String,
        /// `on` or `off`.
        state: String,
    },
    /// Every connection of a download and what it is doing.
    Connections { job: String },
    /// Open the operating system's network settings.
    NetSettings,
    /// Look inside a link without downloading: a torrent's files, a video's formats, a file's size.
    Info { link: String },
    /// Show or install the helpers IDMX uses for video: yt-dlp and ffmpeg.
    Tools {
        /// `install` downloads yt-dlp into IDMX's own folder (checked against its published checksum).
        action: Option<String>,
    },
    /// Check for a newer release.
    Update {
        /// `owner/repo` on GitHub (defaults to the one saved in settings).
        #[arg(long)]
        repo: Option<String>,
    },
    /// Measure each network, then all together, and say how much combining would help.
    Speedtest {
        /// File to pull for the test (defaults to a public speed-test endpoint).
        url: Option<String>,
    },
    /// Look at a URL without downloading it.
    Probe { url: String },
    /// Hash a file.
    Hash {
        file: PathBuf,
        #[arg(long, default_value = "sha256")]
        algo: String,
    },
    /// Run IDMX in the background with its API and web dashboard.
    Daemon {
        #[arg(long, default_value_t = 7733)]
        port: u16,
        /// Listen on every network interface, not just this computer.
        #[arg(long)]
        lan: bool,
        /// Add two *simulated* networks (labelled as such) on top of the real ones, for demos and
        /// UI development on a machine that only has one connection.
        #[arg(long)]
        simulate: bool,
    },
    /// Print the dashboard address of the running IDMX.
    Open,
}

#[derive(Args, Clone, Default)]
struct Fetch {
    /// URLs. Patterns work: `https://x/img[001-120].jpg`, `https://x/{a,b,c}.zip`.
    urls: Vec<String>,
    /// Read URLs from a text file (or `-` for stdin); Metalink (.meta4) files work too.
    #[arg(short = 'f', long = "file")]
    files: Vec<PathBuf>,
    /// Folder to save into.
    #[arg(short = 'd', long)]
    dir: Option<PathBuf>,
    /// File name (single URL only).
    #[arg(short = 'o', long = "output")]
    name: Option<String>,
    /// Use only these networks (comma-separated, see `idmx nets`).
    #[arg(short = 'i', long = "interface", value_delimiter = ',')]
    interfaces: Vec<String>,
    /// Connections per network.
    #[arg(short = 's', long)]
    streams: Option<u32>,
    /// Speed limit for this download, e.g. 5M.
    #[arg(long, value_name = "SIZE/s")]
    limit: Option<String>,
    /// Extra request header, `"Name: value"`. Repeatable.
    #[arg(short = 'H', long = "header")]
    headers: Vec<String>,
    /// Cookie header value, `"a=1; b=2"`.
    #[arg(long)]
    cookie: Option<String>,
    /// Netscape cookies.txt to take cookies from.
    #[arg(long, value_name = "FILE")]
    cookie_file: Option<PathBuf>,
    /// Basic auth, `user:password`.
    #[arg(short = 'u', long)]
    user: Option<String>,
    #[arg(long)]
    bearer: Option<String>,
    #[arg(long)]
    referer: Option<String>,
    #[arg(long)]
    user_agent: Option<String>,
    /// HTTP/SOCKS proxy URL.
    #[arg(long)]
    proxy: Option<String>,
    /// Verify against `sha256:<hex>`, a bare hash, or `auto` to look for a published one.
    #[arg(long)]
    checksum: Option<String>,
    /// Skip your post-download rules.
    #[arg(long)]
    no_post: bool,
    /// Queue it paused.
    #[arg(long)]
    paused: bool,
    #[arg(long, default_value_t = 0)]
    priority: i32,
    /// Start later: `01:30`, `2026-10-05 01:30`, `+2h`.
    #[arg(long)]
    at: Option<String>,
    /// Only run inside the schedule window you set in the app.
    #[arg(long)]
    window: bool,
    /// Video quality for pages with video: best, 1080, 720, 480, audio, audio216, audio320 (MP3 at that kbps) — or a yt-dlp format.
    #[arg(long, value_name = "QUALITY")]
    quality: Option<String>,
    /// Treat the link as a video page even if it doesn't look like one.
    #[arg(long)]
    video: bool,
    /// Download the link as a plain file, never as a video page.
    #[arg(long)]
    plain: bool,
    /// Torrent: fetch only these files (numbers from `idmx info`), e.g. 0,2,3.
    #[arg(long = "files", value_delimiter = ',', value_name = "N")]
    pick: Vec<u32>,
    /// Print JSON lines instead of the live view.
    #[arg(long)]
    json: bool,
    /// Plain progress lines instead of the live view.
    #[arg(long)]
    no_tui: bool,
}

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("--native-host") {
        idmx_server::browser::run_native_host(&idmx_core::default_data_dir());
        return;
    }
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into())).with_writer(std::io::stderr).init();
    let cli = Cli::parse();
    let home = cli.home.unwrap_or_else(idmx_core::default_data_dir);
    let code = match run(cli.cmd, home).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}error:{} {e:#}", view::rgb(248, 113, 113), RESET);
            1
        }
    };
    std::process::exit(code);
}

async fn run(cmd: Cmd, home: PathBuf) -> Result<i32> {
    match cmd {
        Cmd::Get(f) => get(f, home).await,
        Cmd::Add(f) => add(f, home).await,
        Cmd::Ls { json } => ls(home, json).await,
        Cmd::Status { json } => status(home, json).await,
        Cmd::Pause { id } => job_cmd(home, &id, "pause", "paused").await,
        Cmd::Resume { id } => job_cmd(home, &id, "resume", "resumed").await,
        Cmd::Cancel { id } => job_cmd(home, &id, "cancel", "cancelled").await,
        Cmd::Top { id } => job_cmd(home, &id, "top", "moved to the front").await,
        Cmd::Rm { id, delete_file } => rm(home, &id, delete_file).await,
        Cmd::Nets { json, all, no_ping } => nets(json, all, no_ping).await,
        Cmd::Net { job, network, state } => net_switch(home, &job, &network, &state).await,
        Cmd::Connections { job } => connections(home, &job).await,
        Cmd::NetSettings => {
            idmx_core::os::open_network_settings();
            Ok(0)
        }
        Cmd::Update { repo } => update(home, repo).await,
        Cmd::Info { link } => info(home, link).await,
        Cmd::Tools { action } => tools(home, action).await,
        Cmd::Speedtest { url } => speedtest(home, url).await,
        Cmd::Probe { url } => probe(url).await,
        Cmd::Hash { file, algo } => hash(file, &algo),
        Cmd::Daemon { port, lan, simulate } => daemon(home, port, lan, simulate).await,
        Cmd::Open => open(home),
    }
}

fn need_running(home: &std::path::Path) -> Result<Remote> {
    Remote::connect(home).ok_or_else(|| anyhow!("IDMX isn't running. Open the app, or start the background service with `idmx daemon`."))
}

// ───────────────────────────── building requests ─────────────────────────────

fn requests(f: &Fetch) -> Result<Vec<AddRequest>> {
    let mut urls = f.urls.clone();
    for p in &f.files {
        let mut text = String::new();
        if p.as_os_str() == "-" {
            std::io::stdin().read_to_string(&mut text)?;
        } else {
            text = std::fs::read_to_string(p).with_context(|| format!("can't read {}", p.display()))?;
        }
        urls.push(text);
    }
    if urls.is_empty() {
        bail!("give me at least one URL (or -f <file>)");
    }
    let mut headers = Vec::new();
    for h in &f.headers {
        let (k, v) = h.split_once(':').ok_or_else(|| anyhow!("bad header `{h}` — use \"Name: value\""))?;
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    let limit = f.limit.as_deref().map(|s| idmx_core::util::parse_size(s).ok_or_else(|| anyhow!("bad --limit `{s}`"))).transpose()?;
    let auth = f
        .user
        .as_deref()
        .map(|u| {
            let (user, password) = u.split_once(':').ok_or_else(|| anyhow!("--user wants user:password"))?;
            Ok::<_, anyhow::Error>(BasicAuth { user: user.into(), password: password.into() })
        })
        .transpose()?;
    let start_at = f.at.as_deref().map(|s| fmt::parse_when(s).ok_or_else(|| anyhow!("can't read time `{s}` — try 01:30, +2h or 2026-10-05 01:30"))).transpose()?;
    let cookie_txt = f.cookie_file.as_ref().map(|p| std::fs::read_to_string(p).with_context(|| format!("can't read {}", p.display()))).transpose()?;
    if f.name.is_some() && urls.len() > 1 {
        bail!("-o/--output only makes sense with a single URL");
    }

    let base = JobOptions {
        interfaces: (!f.interfaces.is_empty()).then(|| f.interfaces.clone()),
        streams_per_network: f.streams,
        speed_limit: limit,
        headers,
        cookies: f.cookie.clone(),
        auth,
        bearer: f.bearer.clone(),
        referer: f.referer.clone(),
        user_agent: f.user_agent.clone(),
        proxy: f.proxy.clone(),
        checksum: f.checksum.clone(),
        file_name: f.name.clone(),
        scheduled_window: f.window,
        skip_post: f.no_post,
        torrent_files: (!f.pick.is_empty()).then(|| f.pick.clone()),
        media: f.video.then_some(true),
        media_format: f.quality.clone(),
        force_http: f.plain,
        restart_if_changed: false,
    };
    let dest = f.dir.as_ref().map(|d| std::fs::canonicalize(d).unwrap_or_else(|_| d.clone()).to_string_lossy().into_owned());
    Ok(urls
        .into_iter()
        .map(|u| {
            let mut options = base.clone();
            if let Some(txt) = &cookie_txt {
                let c = idmx_core::http::cookies_for(u.trim(), txt);
                if !c.is_empty() {
                    options.cookies = Some(c);
                }
            }
            AddRequest { urls: vec![u], dest_dir: dest.clone(), options, paused: f.paused, priority: f.priority, start_at }
        })
        .collect())
}

// ───────────────────────────── a source of jobs and events ─────────────────────────────

enum Source {
    Local(Engine),
    Remote(Remote),
}

impl Source {
    async fn add(&self, reqs: Vec<AddRequest>) -> Result<Vec<Job>> {
        let mut out = Vec::new();
        for r in reqs {
            match self {
                Source::Local(e) => out.extend(e.add(r)?),
                Source::Remote(c) => out.extend(c.post::<Vec<Job>>("/api/jobs", &r).await?),
            }
        }
        Ok(out)
    }

    async fn jobs(&self) -> Result<Vec<Job>> {
        match self {
            Source::Local(e) => Ok(e.list()),
            Source::Remote(c) => c.jobs().await,
        }
    }

    async fn events(&self) -> Result<mpsc::Receiver<Event>> {
        match self {
            Source::Remote(c) => c.events().await,
            Source::Local(e) => {
                let mut rx = e.subscribe();
                let (tx, out) = mpsc::channel(512);
                tokio::spawn(async move {
                    loop {
                        match rx.recv().await {
                            Ok(ev) => {
                                if tx.send(ev).await.is_err() {
                                    return;
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(_) => return,
                        }
                    }
                });
                Ok(out)
            }
        }
    }

    async fn map(&self, id: &str) -> Option<BlockMap> {
        match self {
            Source::Local(e) => e.block_map(id),
            Source::Remote(c) => c.get::<Option<BlockMap>>(&format!("/api/jobs/{id}/map")).await.ok().flatten(),
        }
    }
}

/// Use a running instance if there is one; otherwise become the engine for this command.
async fn source(home: &std::path::Path) -> Result<Source> {
    if let Some(r) = Remote::connect(home) {
        return Ok(Source::Remote(r));
    }
    match Engine::start(home.to_path_buf()).await {
        Ok(e) => Ok(Source::Local(e)),
        Err(e) => bail!("{e}. If another `idmx` is mid-download, wait for it or use the app."),
    }
}

// ───────────────────────────── get / add ─────────────────────────────

async fn add(f: Fetch, home: PathBuf) -> Result<i32> {
    let remote = need_running(&home)?;
    let json = f.json;
    let jobs = Source::Remote(remote).add(requests(&f)?).await?;
    if json {
        println!("{}", serde_json::to_string(&jobs)?);
    } else {
        for j in &jobs {
            println!("queued {BOLD}{}{RESET}  {DIM}{}{RESET}", j.file_name, j.id);
        }
        if jobs.is_empty() {
            println!("nothing new to queue (already in the list)");
        }
    }
    Ok(0)
}

async fn get(f: Fetch, home: PathBuf) -> Result<i32> {
    let reqs = requests(&f)?;
    let src = source(&home).await?;
    if let Source::Local(e) = &src {
        if !e.is_online() {
            e.shutdown().await;
            bail!("{}", idmx_core::error::NO_NETWORK_MSG.replace(" and IDMX will carry on by itself", ", then run this again"));
        }
    }
    let jobs = src.add(reqs).await?;
    let ids: Vec<String> = if jobs.is_empty() {
        // Everything was already queued: follow those instead.
        let urls: Vec<String> = f.urls.clone();
        src.jobs().await?.into_iter().filter(|j| urls.contains(&j.url)).map(|j| j.id).collect()
    } else {
        jobs.iter().map(|j| j.id.clone()).collect()
    };
    if ids.is_empty() {
        bail!("nothing to download");
    }
    if f.paused {
        println!("queued paused: {}", ids.join(", "));
        return Ok(0);
    }
    let code = follow(&src, ids, &f).await?;
    if let Source::Local(e) = &src {
        e.shutdown().await;
    }
    Ok(code)
}

/// Some pseudo-terminals report 0 columns; never trust a width too small to draw in.
fn term_width() -> usize {
    crossterm::terminal::size().map(|s| s.0 as usize).ok().filter(|w| *w >= 40).unwrap_or(100)
}

async fn follow(src: &Source, ids: Vec<String>, f: &Fetch) -> Result<i32> {
    let tty = std::io::stdout().is_terminal() && !f.no_tui && !f.json;
    let mut view = View::default();
    for j in src.jobs().await? {
        if ids.contains(&j.id) {
            view.jobs.insert(j.id.clone(), j);
        }
    }
    let mut rx = src.events().await?;
    let mut painter = tty.then(Painter::new);
    let mut draw = tokio::time::interval(Duration::from_millis(100));
    let mut maps = tokio::time::interval(Duration::from_millis(400));
    let mut plain = tokio::time::interval(Duration::from_secs(1));
    let started = Instant::now();
    let mut interrupted = false;

    // A seeding torrent has everything; there's nothing left to wait for.
    let finished = |v: &View| v.jobs.values().all(|j| j.status.is_finished() || matches!(j.status, JobStatus::Paused | JobStatus::Seeding));

    loop {
        if finished(&view) && !view.jobs.is_empty() {
            break;
        }
        tokio::select! {
            ev = rx.recv() => {
                let Some(ev) = ev else { break };
                apply(&mut view, ev, f.json);
            }
            _ = draw.tick(), if tty => {
                let w = term_width();
                if let Some(p) = painter.as_mut() { p.draw(&frame(&view, w, &ids)); }
            }
            _ = maps.tick(), if tty => {
                for (id, j) in view.jobs.clone() {
                    if j.status == JobStatus::Downloading {
                        if let Some(m) = src.map(&id).await { view.maps.insert(id, m); }
                    }
                }
            }
            _ = plain.tick(), if !tty && !f.json => {
                for j in view.jobs.values().filter(|j| j.status == JobStatus::Downloading) {
                    println!("{} {} {} {} eta {}", j.id, fmt::clip(&j.file_name, 40), fmt::pct(j.downloaded, j.size), fmt::rate(j.speed), fmt::eta(j.eta));
                }
            }
            _ = tokio::signal::ctrl_c() => { interrupted = true; break; }
        }
    }
    // One last frame so the final state stays on screen.
    if let Some(p) = painter.as_mut() {
        let w = term_width();
        p.draw(&frame(&view, w, &ids));
    }
    drop(painter);

    if interrupted {
        match src {
            Source::Local(e) => {
                e.shutdown().await;
                println!("\nPaused. Run the same command again to resume where it stopped.");
            }
            Source::Remote(_) => println!("\nDetached — the downloads keep going in IDMX. `idmx ls` to check on them."),
        }
        return Ok(130);
    }

    let mut code = 0;
    for j in view.jobs.values() {
        match j.status {
            JobStatus::Completed => {
                if f.json {
                    println!("{}", serde_json::json!({"event": "completed", "id": j.id, "path": j.path, "size": j.size}));
                } else {
                    summary(j, started.elapsed());
                }
            }
            JobStatus::Paused => code = code.max(2),
            JobStatus::Seeding => {
                summary(j, started.elapsed());
                println!("  {DIM}It's still sharing while this runs. Run `idmx daemon` (or keep the app open) to keep seeding after a download.{RESET}");
            }
            _ => {
                code = 1;
                if f.json {
                    println!("{}", serde_json::json!({"event": "failed", "id": j.id, "error": j.error}));
                }
            }
        }
    }
    Ok(code)
}

fn apply(view: &mut View, ev: Event, json: bool) {
    match ev {
        Event::JobAdded(j) | Event::JobUpdated(j) => {
            if view.jobs.contains_key(&j.id) {
                if json {
                    println!("{}", serde_json::json!({"event": "status", "id": j.id, "status": j.status, "error": j.error}));
                }
                view.jobs.insert(j.id.clone(), *j);
            }
        }
        Event::Tick { jobs, .. } => {
            for t in jobs {
                if let Some(j) = view.jobs.get_mut(&t.id) {
                    j.downloaded = t.downloaded;
                    j.speed = t.speed;
                    j.eta = t.eta;
                    j.networks = t.networks;
                    if json {
                        println!("{}", serde_json::json!({"event": "progress", "id": j.id, "downloaded": j.downloaded, "size": j.size, "speed": j.speed, "eta": j.eta}));
                    }
                }
            }
        }
        Event::JobRemoved(id) => {
            view.jobs.remove(&id);
        }
        _ => {}
    }
}

fn summary(j: &Job, elapsed: Duration) {
    let size = j.size.unwrap_or(j.downloaded);
    let secs = j.completed_at.zip(j.started_at).map(|(c, s)| (c - s).max(0) as f64).filter(|s| *s > 0.0).unwrap_or(elapsed.as_secs_f64().max(0.1));
    println!(
        "{}✔{} {BOLD}{}{RESET}  {} in {} {DIM}({}){RESET}",
        view::rgb(52, 211, 153),
        RESET,
        j.file_name,
        fmt::bytes(size),
        fmt::eta(Some(secs.round() as u64)),
        fmt::rate(size as f64 / secs)
    );
    if let Some(p) = &j.path {
        println!("  {DIM}{p}{RESET}");
    }
    if let Some(c) = &j.checksum {
        match (c.verified, &c.actual) {
            (Some(true), _) => println!("  {}✓ {} verified{} {DIM}({}){RESET}", view::rgb(52, 211, 153), c.algo, RESET, c.source.clone().unwrap_or_default()),
            (_, Some(a)) => println!("  {DIM}{}: {a}{RESET}", c.algo),
            _ => {}
        }
    }
}

// ───────────────────────────── ls / status / job commands ─────────────────────────────

async fn ls(home: PathBuf, json: bool) -> Result<i32> {
    let jobs = match Remote::connect(&home) {
        Some(r) => r.jobs().await?,
        // Not running: read what was saved.
        None => idmx_core::store::State::load(&home.join("state.json")).jobs.into_iter().map(|s| s.job).collect(),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&jobs)?);
        return Ok(0);
    }
    if jobs.is_empty() {
        println!("no downloads");
        return Ok(0);
    }
    println!("{DIM}{:<9} {:<12} {:>7} {:>11} {:>10}  NAME{RESET}", "ID", "STATUS", "DONE", "SPEED", "SIZE");
    for j in &jobs {
        let (label, c) = match j.status {
            JobStatus::Completed => ("done", (52, 211, 153)),
            JobStatus::Failed => ("failed", (248, 113, 113)),
            JobStatus::Downloading => ("downloading", (64, 200, 255)),
            JobStatus::Paused => ("paused", (251, 191, 36)),
            JobStatus::Queued => ("queued", (156, 163, 175)),
            JobStatus::Cancelled => ("cancelled", (156, 163, 175)),
            JobStatus::Probing => ("probing", (251, 191, 36)),
            JobStatus::Verifying => ("verifying", (129, 140, 248)),
            JobStatus::Processing => ("processing", (129, 140, 248)),
            JobStatus::Seeding => ("seeding", (52, 211, 153)),
        };
        println!(
            "{:<9} {}{:<12}{RESET} {:>7} {:>11} {:>10}  {}",
            j.id,
            view::rgb(c.0, c.1, c.2),
            label,
            fmt::pct(j.downloaded, j.size).trim(),
            if j.status == JobStatus::Downloading { fmt::rate(j.speed) } else { "".into() },
            j.size.map(fmt::bytes).unwrap_or_default(),
            fmt::clip(&j.file_name, 48),
        );
    }
    Ok(0)
}

async fn status(home: PathBuf, json: bool) -> Result<i32> {
    let r = need_running(&home)?;
    let s: serde_json::Value = r.get("/api/state").await?;
    let st: GlobalStats = serde_json::from_value(s["stats"].clone())?;
    if json {
        println!("{}", serde_json::to_string_pretty(&s["stats"])?);
    } else {
        println!(
            "{BOLD}{}{RESET}  {} active · {} queued · {} done · {} failed   {DIM}session {} · lifetime {}{RESET}",
            fmt::rate(st.speed),
            st.active,
            st.queued,
            st.completed,
            st.failed,
            fmt::bytes(st.session_bytes),
            fmt::bytes(st.total_bytes)
        );
    }
    Ok(0)
}

async fn job_cmd(home: PathBuf, id: &str, verb: &str, done: &str) -> Result<i32> {
    let r = need_running(&home)?;
    if id == "all" {
        match verb {
            "pause" => r.post::<serde_json::Value>("/api/pause-all", &serde_json::json!({})).await?,
            "resume" => r.post::<serde_json::Value>("/api/resume-all", &serde_json::json!({})).await?,
            _ => bail!("`all` only works with pause and resume"),
        };
        println!("all downloads {done}");
        return Ok(0);
    }
    // Accept a unique prefix of an id, like git.
    let id = resolve_id(&r, id).await?;
    r.post::<serde_json::Value>(&format!("/api/jobs/{id}/{verb}"), &serde_json::json!({})).await?;
    println!("{id} {done}");
    Ok(0)
}

async fn rm(home: PathBuf, id: &str, delete_file: bool) -> Result<i32> {
    let r = need_running(&home)?;
    let id = resolve_id(&r, id).await?;
    r.delete::<serde_json::Value>(&format!("/api/jobs/{id}?deleteFile={delete_file}")).await?;
    println!("{id} removed");
    Ok(0)
}

async fn resolve_id(r: &Remote, given: &str) -> Result<String> {
    let jobs = r.jobs().await?;
    let m: Vec<&Job> = jobs.iter().filter(|j| j.id.starts_with(given)).collect();
    match m.len() {
        0 => bail!("no download with id `{given}` (try `idmx ls`)"),
        1 => Ok(m[0].id.clone()),
        _ => bail!("`{given}` matches {} downloads — type more of the id", m.len()),
    }
}

// ───────────────────────────── nets / speedtest / probe / hash ─────────────────────────────

async fn nets(json: bool, all: bool, no_ping: bool) -> Result<i32> {
    let list = tokio::task::spawn_blocking(idmx_core::net::discover).await?;
    // Latency to each usable network, all at once.
    let pings: std::collections::HashMap<String, Option<f64>> = if no_ping {
        Default::default()
    } else {
        let tasks: Vec<_> = list
            .iter()
            .filter(|n| n.usable)
            .cloned()
            .map(|n| tokio::spawn(async move { let ms = idmx_core::ping::ping(&n, true).await; (n.id, ms) }))
            .collect();
        let mut m = std::collections::HashMap::new();
        for t in tasks {
            if let Ok((id, ms)) = t.await {
                m.insert(id, ms);
            }
        }
        m
    };
    if json {
        let out: Vec<serde_json::Value> = list
            .iter()
            .map(|n| {
                let mut v = serde_json::to_value(n).unwrap_or_default();
                v["latencyMs"] = pings.get(&n.id).copied().flatten().map(serde_json::Value::from).unwrap_or(serde_json::Value::Null);
                v["reachable"] = pings.get(&n.id).map(|p| serde_json::Value::from(p.is_some())).unwrap_or(serde_json::Value::Null);
                v
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(0);
    }
    if !list.iter().any(|n| n.usable) {
        println!("{}No network connection.{RESET} Connect to Wi-Fi or plug in a cable, then run this again.\n  (`idmx nets --all` lists everything the OS reports.)", view::rgb(248, 113, 113));
        if !all {
            return Ok(1);
        }
    }
    println!("{DIM}{:<10} {:<22} {:<10} {:<18} {:<8} {:<16} {}{RESET}", "ID", "NAME", "KIND", "ADDRESS", "PING", "GATEWAY", "NOTES");
    for n in list.iter().filter(|n| n.kind != IfKind::Loopback && (all || n.usable)) {
        let c = view::net_color(n.kind);
        let mut notes = Vec::new();
        if !n.usable {
            notes.push("not usable".to_string());
        } else if !n.kind.default_enabled() {
            notes.push("off by default".to_string());
        }
        let others = |same: &dyn Fn(&NetInterface) -> bool| -> Vec<&str> { list.iter().filter(|o| o.id != n.id && o.usable && n.usable && same(o)).map(|o| o.id.as_str()).collect() };
        let shares_gw = others(&|o| n.gateway.is_some() && o.gateway == n.gateway);
        let shares_sub = others(&|o| n.subnets.iter().any(|s| o.subnets.contains(s)));
        if !shares_sub.is_empty() {
            notes.push(format!("same subnet as {} — one shared link", shares_sub.join(", ")));
        } else if !shares_gw.is_empty() {
            notes.push(format!("same gateway as {}", shares_gw.join(", ")));
        }
        let ping = match pings.get(&n.id) {
            Some(Some(ms)) => format!("{ms:.0} ms"),
            Some(None) => "no net".to_string(),
            None => "—".to_string(),
        };
        println!(
            "{}{:<10}{RESET} {:<22} {:<10} {:<18} {:<8} {:<16} {DIM}{}{RESET}",
            view::rgb(c.0, c.1, c.2),
            n.id,
            fmt::clip(&n.label, 22),
            format!("{:?}", n.kind).to_lowercase(),
            n.ipv4.first().or(n.ipv6.first()).cloned().unwrap_or_else(|| "—".into()),
            ping,
            n.gateway.clone().unwrap_or_else(|| "—".into()),
            notes.join(" · ")
        );
    }
    Ok(0)
}

async fn net_switch(home: PathBuf, job: &str, network: &str, state: &str) -> Result<i32> {
    let on = match state {
        "on" | "enable" | "true" | "1" => true,
        "off" | "disable" | "false" | "0" => false,
        _ => bail!("say `on` or `off`"),
    };
    let r = need_running(&home)?;
    let id = resolve_id(&r, job).await?;
    r.post::<serde_json::Value>(&format!("/api/jobs/{id}/networks/{network}"), &serde_json::json!({ "enabled": on })).await?;
    println!("{network} {} for {id}", if on { "on" } else { "off" });
    Ok(0)
}

async fn connections(home: PathBuf, job: &str) -> Result<i32> {
    let r = need_running(&home)?;
    let id = resolve_id(&r, job).await?;
    let list: Vec<StreamStat> = r.get(&format!("/api/jobs/{id}/streams")).await?;
    if list.is_empty() {
        println!("no open connections (is it downloading?)");
        return Ok(0);
    }
    println!("{DIM}{:>4}  {:<14} {:<12} {:>7} {:>11} {:>10}{RESET}", "#", "NETWORK", "STATE", "CHUNK", "SPEED", "TOTAL");
    for s in &list {
        println!(
            "{:>4}  {:<14} {:<12} {:>7} {:>11} {:>10}",
            s.id,
            fmt::clip(&s.net, 14),
            s.state,
            s.block.map(|b| format!("#{b}")).unwrap_or_default(),
            if s.speed > 1.0 { fmt::rate(s.speed) } else { String::new() },
            fmt::bytes(s.bytes)
        );
    }
    Ok(0)
}

async fn info(home: PathBuf, link: String) -> Result<i32> {
    let src = source(&home).await?;
    let is_torrent = idmx_core::torrent::Source::parse(&link).is_some();
    let out: Result<i32> = async {
        if is_torrent {
            println!("{DIM}looking inside the torrent — a magnet link has to ask other people for this, which can take a little while…{RESET}");
            let m: TorrentMeta = match &src {
                Source::Local(e) => e.torrent_inspect(&link).await?,
                Source::Remote(r) => r.post("/api/torrent/inspect", &serde_json::json!({ "url": link })).await?,
            };
            println!("{BOLD}{}{RESET}  {DIM}{}{RESET}", m.name, fmt::bytes(m.total));
            println!("  {DIM}info hash {}{RESET}", m.info_hash);
            for f in &m.files {
                println!("  {:>3}  {:>10}  {}", f.index, fmt::bytes(f.size), f.path);
            }
            if m.files.len() > 1 {
                println!("\n{DIM}Download only some with:  idmx get \"{}\" --files 0,2{RESET}", fmt::clip(&link, 40));
            }
            return Ok(0);
        }
        // A video page? Ask yt-dlp. Otherwise it's a plain file.
        if idmx_core::media::looks_like_media_page(&link) {
            let p: MediaProbe = match &src {
                Source::Local(e) => e.media_probe(&link, &JobOptions::default()).await?,
                Source::Remote(r) => r.post("/api/media/probe", &serde_json::json!({ "url": link })).await?,
            };
            println!("{BOLD}{}{RESET}", p.title);
            if let Some(u) = &p.uploader {
                println!("  by          {u}");
            }
            if let Some(d) = p.duration {
                println!("  length      {}", fmt::eta(Some(d)));
            }
            println!("  best        {}{}", p.format, p.size.map(|s| format!("  ({})", fmt::bytes(s))).unwrap_or_default());
            if !p.heights.is_empty() {
                println!("  qualities   {}", p.heights.iter().map(|h| format!("{h}p")).collect::<Vec<_>>().join("  "));
            }
            println!("  fetched by  {}", if p.direct { "IDMX, across all your networks" } else { "yt-dlp (a streaming format)" });
            return Ok(0);
        }
        probe(link.clone()).await
    }
    .await;
    if let Source::Local(e) = &src {
        e.shutdown().await;
    }
    out
}

async fn tools(home: PathBuf, action: Option<String>) -> Result<i32> {
    let src = source(&home).await?;
    let status = |s: &ToolsStatus| {
        for (name, t) in [("yt-dlp", &s.ytdlp), ("javascript", &s.js), ("ffmpeg", &s.ffmpeg)] {
            match t {
                Some(t) => println!("  {}✓{RESET} {BOLD}{name}{RESET} {}  {DIM}{}{}{RESET}", view::rgb(52, 211, 153), t.version, t.path, if t.managed { "  (installed by IDMX)" } else { "" }),
                None => println!("  {}✗{RESET} {BOLD}{name}{RESET} {DIM}not found{RESET}", view::rgb(251, 191, 36)),
            }
        }
    };
    let code = match (action.as_deref(), &src) {
        (Some("install"), Source::Local(e)) => {
            let mut ev = e.subscribe();
            e.install_tool("all")?;
            println!("Installing yt-dlp, a JavaScript runtime and ffmpeg…");
            let mut ok = false;
            loop {
                match tokio::time::timeout(Duration::from_secs(180), ev.recv()).await {
                    Ok(Ok(Event::Tools(t))) => {
                        if let Some(i) = &t.installing {
                            if let Some(err) = &i.error {
                                eprintln!("{}error:{RESET} {err}", view::rgb(248, 113, 113));
                                break;
                            }
                            print!("\r  {:<8} {} of {}   ", i.tool, fmt::bytes(i.downloaded), i.total.map(fmt::bytes).unwrap_or_else(|| "?".into()));
                            use std::io::Write;
                            let _ = std::io::stdout().flush();
                        } else if t.ytdlp.is_some() && t.ffmpeg.is_some() && t.js.is_some() {
                            println!();
                            ok = true;
                            break;
                        }
                    }
                    // Any other event is just the engine going about its business.
                    Ok(Ok(_)) | Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
                    _ => break,
                }
            }
            status(&e.tools_status(true).await);
            if ok { 0 } else { 1 }
        }
        (Some("install"), Source::Remote(r)) => {
            r.post::<serde_json::Value>("/api/tools/install/all", &serde_json::json!({})).await?;
            println!("Installing yt-dlp in the running IDMX…");
            for _ in 0..90 {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let s: ToolsStatus = r.get("/api/tools").await?;
                if let Some(e) = s.installing.as_ref().and_then(|i| i.error.clone()) {
                    bail!("{e}");
                }
                if s.installing.is_none() && s.ytdlp.is_some() && s.js.is_some() && s.ffmpeg.is_some() {
                    status(&s);
                    return Ok(0);
                }
            }
            bail!("it's taking too long; check the IDMX app")
        }
        (None, Source::Local(e)) => {
            status(&e.tools_status(true).await);
            0
        }
        (None, Source::Remote(r)) => {
            status(&r.post::<ToolsStatus>("/api/tools/refresh", &serde_json::json!({})).await?);
            0
        }
        (Some(other), _) => bail!("unknown action `{other}` — try `idmx tools` or `idmx tools install`"),
    };
    if let Source::Local(e) = &src {
        e.shutdown().await;
    }
    Ok(code)
}

async fn update(home: PathBuf, repo: Option<String>) -> Result<i32> {
    let repo = repo.unwrap_or_else(|| idmx_core::store::State::load(&home.join("state.json")).settings.update_repo);
    if repo.trim().is_empty() {
        bail!("no update source set — pass --repo owner/name, or set one in Settings");
    }
    let client = idmx_core::http::build_client(&Default::default())?;
    match idmx_core::update::check(&client, &repo, env!("CARGO_PKG_VERSION")).await? {
        Some(u) => {
            println!("{BOLD}IDMX {}{RESET} is available (you have {})\n  {}", u.version, env!("CARGO_PKG_VERSION"), u.url);
            Ok(10)
        }
        None => {
            println!("You're up to date ({}).", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
    }
}

async fn speedtest(home: PathBuf, url: Option<String>) -> Result<i32> {
    let src = source(&home).await?;
    let mut rx = src.events().await?;
    match &src {
        Source::Local(e) => e.speed_test(url)?,
        Source::Remote(r) => {
            r.post::<serde_json::Value>("/api/speedtest", &serde_json::json!({ "url": url })).await?;
        }
    }
    let mut painter = std::io::stdout().is_terminal().then(Painter::new);
    let mut last: Option<SpeedTestReport> = None;
    while let Some(ev) = tokio::time::timeout(Duration::from_secs(90), rx.recv()).await.ok().flatten() {
        if let Event::SpeedTest(r) = ev {
            let lines = speed_lines(&r);
            match painter.as_mut() {
                Some(p) => p.draw(&lines),
                None if r.done => println!("{}", lines.join("\n")),
                None => {}
            }
            let done = r.done;
            last = Some(r);
            if done {
                break;
            }
        }
    }
    drop(painter);
    if let Source::Local(e) = &src {
        e.shutdown().await;
    }
    let Some(r) = last else { bail!("the speed test didn't finish") };
    Ok(if r.phase == "failed" { 1 } else { 0 })
}

fn speed_lines(r: &SpeedTestReport) -> Vec<String> {
    let mut out = vec![format!("{BOLD}{}{RESET}", r.phase)];
    let max = r.nets.iter().map(|n| n.mbps).fold(r.combined_mbps.unwrap_or(0.0), f64::max).max(1.0);
    for n in &r.nets {
        let c = view::net_color(n.kind);
        let w = ((n.mbps / max) * 30.0).round() as usize;
        out.push(format!(
            "  {} {:<16} {}{}{RESET}{DIM}{}{RESET} {:>8.1} Mbit/s {}",
            view::rgb(c.0, c.1, c.2) + "●" + RESET,
            fmt::clip(&n.label, 16),
            view::rgb(c.0, c.1, c.2),
            "█".repeat(w),
            "░".repeat(30 - w.min(30)),
            n.mbps,
            n.error.clone().map(|e| format!("{}✗ {e}{RESET}", view::rgb(248, 113, 113))).or(n.latency_ms.map(|l| format!("{DIM}{l:.0} ms{RESET}"))).unwrap_or_default()
        ));
    }
    if let Some(c) = r.combined_mbps {
        let w = ((c / max) * 30.0).round() as usize;
        out.push(format!("  {BOLD}◆ {:<16}{RESET} {}{}{RESET}{DIM}{}{RESET} {:>8.1} Mbit/s", "Combined", view::rgb(255, 255, 255), "█".repeat(w.min(30)), "░".repeat(30 - w.min(30)), c));
        if let Some(g) = r.gain {
            out.push(format!("  {BOLD}{g:.2}×{RESET} the fastest single network"));
        }
    }
    for w in &r.warnings {
        out.push(format!("  {}! {w}{RESET}", view::rgb(251, 191, 36)));
    }
    out
}

async fn probe(url: String) -> Result<i32> {
    let client = idmx_core::http::build_client(&Default::default())?;
    let headers = idmx_core::http::base_headers(&JobOptions::default(), None);
    let (i, _) = idmx_core::http::probe(&client, &url, &headers).await?;
    println!("{BOLD}{}{RESET}", i.file_name);
    println!("  size        {}", i.size.map(|s| format!("{} ({s} bytes)", fmt::bytes(s))).unwrap_or_else(|| "unknown".into()));
    println!("  ranges      {}", if i.ranges { "yes — can use several connections and resume" } else { "no — single connection only" });
    println!("  type        {}", i.mime.clone().unwrap_or_else(|| "?".into()));
    println!("  etag        {}", i.etag.clone().unwrap_or_else(|| "—".into()));
    println!("  modified    {}", i.last_modified.clone().unwrap_or_else(|| "—".into()));
    println!("  first byte  {} ms", i.ttfb_ms);
    if i.final_url != url {
        println!("  redirected  {}", i.final_url);
    }
    Ok(0)
}

fn hash(file: PathBuf, algo: &str) -> Result<i32> {
    let a = idmx_core::verify::Algo::parse(algo).ok_or_else(|| anyhow!("unknown algorithm `{algo}` (sha256, sha1, md5, blake3)"))?;
    let h = idmx_core::verify::hash_file(&file, a, |_| {})?;
    println!("{h}  {}", file.display());
    Ok(0)
}

// ───────────────────────────── daemon / open ─────────────────────────────

/// Two extra networks that reuse the primary connection's address. They exercise every code
/// path of multi-network downloading, but of course add no real bandwidth.
fn simulated(real: &[NetInterface]) -> Vec<NetInterface> {
    let Some(base) = real.iter().find(|n| n.usable && n.kind != IfKind::Vpn).cloned() else { return vec![] };
    let mk = |id: &str, label: &str, kind: IfKind| NetInterface { id: id.into(), label: label.into(), kind, gateway: Some(format!("sim-{id}")), mac: None, link_mbps: None, ..base.clone() };
    vec![mk("sim-usb", "iPhone USB (simulated)", IfKind::Tether), mk("sim-cell", "Cellular (simulated)", IfKind::Cellular)]
}

async fn daemon(home: PathBuf, port: u16, lan: bool, simulate: bool) -> Result<i32> {
    let mut simulated_base: Vec<NetInterface> = Vec::new();
    let opts = if simulate {
        let mut nets = tokio::task::spawn_blocking(idmx_core::net::discover).await?;
        let sims = simulated(&nets);
        nets.extend(sims);
        simulated_base = nets.clone();
        EngineOptions { fixed_interfaces: Some(nets), no_device_binding: true, force_ping: true, ..Default::default() }
    } else {
        EngineOptions::default()
    };
    let opts = EngineOptions { update_url: std::env::var("IDMX_UPDATE_URL").ok().filter(|u| !u.is_empty()), ..opts };
    let engine = Engine::start_with(home.clone(), opts).await.map_err(|e| anyhow!("{e} — is the app or another daemon already running?"))?;
    if simulate {
        println!("{}! SIMULATED networks are on: two extra networks reuse your real connection. For demos only.{RESET}", view::rgb(251, 191, 36));
    }
    let bind = if lan { std::net::IpAddr::from([0, 0, 0, 0]) } else { std::net::IpAddr::from([127, 0, 0, 1]) };
    let handle = idmx_server::serve(engine.clone(), idmx_server::ServeConfig { bind, port, token: String::new() }).await?;
    println!("{BOLD}IDMX{RESET} is running.");
    println!("  dashboard  {}", handle.dashboard_url());
    println!("  data       {}", home.display());
    if lan {
        println!("  {}! listening on every network. Anyone with the token can control your downloads.{RESET}", view::rgb(251, 191, 36));
    }
    println!("{DIM}Ctrl-C to stop; downloads in progress are paused and resume next time.{RESET}");
    if simulate {
        println!("{DIM}Demo console — type: offline · online · plug · unplug{RESET}");
        spawn_demo_console(engine.clone(), simulated_base.clone());
    }
    let mut events = engine.subscribe();
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            ev = events.recv() => {
                if let Ok(Event::Notice { level, message }) = ev {
                    match level.as_str() {
                        "complete" => println!("✔ {message}"),
                        "error" => eprintln!("✗ {message}"),
                        _ => {}
                    }
                } else if let Ok(Event::Action { action, .. }) = ev {
                    if action == "quit" { break; }
                }
            }
        }
    }
    println!("\nstopping…");
    engine.shutdown().await;
    drop(handle);
    Ok(0)
}

/// Typing into a `--simulate` daemon plays with the network list, to show how IDMX reacts:
/// `offline` / `online` (every network), `unplug` / `plug` (one extra Ethernet).
fn spawn_demo_console(engine: Engine, full: Vec<NetInterface>) {
    use tokio::io::AsyncBufReadExt;
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
        let mut extra;
        while let Ok(Some(line)) = lines.next_line().await {
            let mut nets = full.clone();
            match line.trim() {
                "offline" => {
                    nets.clear();
                    extra = false;
                }
                "online" | "unplug" => extra = false,
                "plug" => extra = true,
                _ => continue,
            }
            if extra {
                if let Some(base) = full.iter().find(|n| n.usable).cloned() {
                    nets.push(NetInterface { id: "sim-eth".into(), label: "Ethernet (simulated)".into(), kind: IfKind::Ethernet, gateway: Some("sim-eth".into()), ..base });
                }
            }
            engine.simulate_networks(nets);
            println!("  → {}", line.trim());
        }
    });
}

fn open(home: PathBuf) -> Result<i32> {
    let r = need_running(&home)?;
    println!("{}/#token={}", r.base(), r.token());
    Ok(0)
}
