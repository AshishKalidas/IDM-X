//! What happens after a download lands: move it, unpack it, run something, tell someone.

use crate::types::{Event, Job, PostAction, PostRule, RuleMatch};
use crate::util::{extension, split_ext, unique_name};
use chrono::Datelike;
use regex::RegexBuilder;
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;

pub fn rule_matches(m: &RuleMatch, job: &Job, name: &str) -> bool {
    let ext = extension(name);
    if !m.extensions.is_empty() && !m.extensions.iter().any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(&ext)) {
        return false;
    }
    if let Some(re) = &m.name_regex {
        match RegexBuilder::new(re).case_insensitive(true).size_limit(1 << 20).build() {
            Ok(r) if r.is_match(name) => {}
            _ => return false,
        }
    }
    if let Some(h) = &m.host {
        let host = Url::parse(&job.url).ok().and_then(|u| u.host_str().map(str::to_ascii_lowercase)).unwrap_or_default();
        let h = h.to_ascii_lowercase();
        if !(host == h || host.ends_with(&format!(".{h}"))) {
            return false;
        }
    }
    let size = job.size.unwrap_or(0);
    if m.min_size.is_some_and(|s| size < s) || m.max_size.is_some_and(|s| size > s) {
        return false;
    }
    true
}

/// `{ext} {host} {year} {month} {date} {name} {stem}` and a leading `~`.
pub fn expand_template(t: &str, job: &Job, name: &str) -> String {
    let now = chrono::Local::now();
    let host = Url::parse(&job.url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
    let (stem, _) = split_ext(name);
    let s = t
        .replace("{ext}", &extension(name))
        .replace("{host}", &host)
        .replace("{year}", &now.year().to_string())
        .replace("{month}", &format!("{:02}", now.month()))
        .replace("{date}", &now.format("%Y-%m-%d").to_string())
        .replace("{name}", name)
        .replace("{stem}", stem);
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(h) = directories::UserDirs::new() {
            return h.home_dir().join(rest).to_string_lossy().into_owned();
        }
    }
    s
}

pub struct PostOutcome {
    pub path: PathBuf,
    pub log: Vec<String>,
}

/// Apply every enabled matching rule, in order. A failing action is logged and skipped —
/// the download itself already succeeded.
pub async fn apply(rules: &[PostRule], job: &Job, mut path: PathBuf, sha256: Option<&str>, emit: &(dyn Fn(Event) + Send + Sync)) -> PostOutcome {
    let mut log = Vec::new();
    for rule in rules.iter().filter(|r| r.enabled) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if !rule_matches(&rule.when, job, &name) {
            continue;
        }
        log.push(format!("rule “{}”", rule.name));
        for action in &rule.actions {
            let r = run_action(action, job, &path, sha256, emit).await;
            match r {
                Ok(ActionResult { new_path, note }) => {
                    if let Some(p) = new_path {
                        path = p;
                    }
                    if let Some(n) = note {
                        log.push(n);
                    }
                }
                Err(e) => log.push(format!("  ✗ {e}")),
            }
        }
    }
    PostOutcome { path, log }
}

struct ActionResult {
    new_path: Option<PathBuf>,
    note: Option<String>,
}

async fn run_action(a: &PostAction, job: &Job, path: &Path, sha256: Option<&str>, emit: &(dyn Fn(Event) + Send + Sync)) -> Result<ActionResult, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match a {
        PostAction::MoveTo { dir } => {
            let dest_dir = PathBuf::from(expand_template(dir, job, &name));
            let (p, d) = (path.to_path_buf(), dest_dir.clone());
            let moved = tokio::task::spawn_blocking(move || move_file(&p, &d)).await.map_err(|e| e.to_string())??;
            Ok(ActionResult { note: Some(format!("  → moved to {}", moved.display())), new_path: Some(moved) })
        }
        PostAction::Extract { dir, delete_archive } => {
            let target = match dir {
                Some(d) => PathBuf::from(expand_template(d, job, &name)),
                None => path.parent().unwrap_or(Path::new(".")).join(strip_archive_ext(&name)),
            };
            let (p, t, del) = (path.to_path_buf(), target.clone(), *delete_archive);
            let count = tokio::task::spawn_blocking(move || extract(&p, &t, del)).await.map_err(|e| e.to_string())??;
            Ok(ActionResult { new_path: None, note: Some(format!("  → extracted {count} files to {}", target.display())) })
        }
        PostAction::Run { command } => {
            let out = run_command(command, job, path, sha256).await?;
            Ok(ActionResult { new_path: None, note: Some(format!("  → ran `{}`{}", short(command), out)) })
        }
        PostAction::Reveal => {
            emit(Event::Action { action: "reveal".into(), path: Some(path.to_string_lossy().into_owned()), job_id: Some(job.id.clone()) });
            Ok(ActionResult { new_path: None, note: None })
        }
        PostAction::Notify { message } => {
            let m = message.clone().unwrap_or_else(|| format!("{name} is ready"));
            emit(Event::Notice { level: "info".into(), message: m });
            Ok(ActionResult { new_path: None, note: None })
        }
    }
}

fn short(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() > 48 { format!("{}…", s.chars().take(48).collect::<String>()) } else { s.to_string() }
}

fn move_file(path: &Path, dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    let name = path.file_name().ok_or("no file name")?.to_string_lossy().into_owned();
    let unique = unique_name(dir, &name, |_| false);
    let to = dir.join(unique);
    if std::fs::rename(path, &to).is_err() {
        // Different volume: copy then remove.
        std::fs::copy(path, &to).map_err(|e| e.to_string())?;
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    Ok(to)
}

fn strip_archive_ext(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    for e in [".tar.gz", ".tgz", ".tar.xz", ".tar", ".zip"] {
        if lower.ends_with(e) {
            return name[..name.len() - e.len()].to_string();
        }
    }
    split_ext(name).0.to_string()
}

pub fn is_archive(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    [".zip", ".tar", ".tar.gz", ".tgz"].iter().any(|e| l.ends_with(e))
}

fn extract(archive: &Path, into: &Path, delete: bool) -> Result<usize, String> {
    let name = archive.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    std::fs::create_dir_all(into).map_err(|e| e.to_string())?;
    let count = if name.ends_with(".zip") {
        let f = std::fs::File::open(archive).map_err(|e| e.to_string())?;
        let mut z = zip::ZipArchive::new(f).map_err(|e| format!("not a valid zip: {e}"))?;
        let mut n = 0;
        for i in 0..z.len() {
            let mut entry = z.by_index(i).map_err(|e| e.to_string())?;
            // `enclosed_name` refuses absolute paths and `..` — the classic zip-slip.
            let Some(rel) = entry.enclosed_name() else { continue };
            let out = into.join(rel);
            if entry.is_dir() {
                std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
                continue;
            }
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            let mut f = std::fs::File::create(&out).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut f).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            if let Some(mode) = entry.unix_mode() {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o777));
            }
            n += 1;
        }
        n
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") || name.ends_with(".tar") {
        let f = std::fs::File::open(archive).map_err(|e| e.to_string())?;
        let reader: Box<dyn std::io::Read> = if name.ends_with(".tar") { Box::new(f) } else { Box::new(flate2::read::GzDecoder::new(f)) };
        let mut tar = tar::Archive::new(reader);
        let mut n = 0;
        for entry in tar.entries().map_err(|e| e.to_string())? {
            let mut e = entry.map_err(|e| e.to_string())?;
            // `unpack_in` refuses paths that escape the destination.
            if e.unpack_in(into).map_err(|e| e.to_string())? && e.header().entry_type().is_file() {
                n += 1;
            }
        }
        n
    } else {
        return Err(format!("don't know how to extract {name}"));
    };
    if delete {
        let _ = std::fs::remove_file(archive);
    }
    Ok(count)
}

async fn run_command(cmd: &str, job: &Job, path: &Path, sha256: Option<&str>) -> Result<String, String> {
    #[cfg(windows)]
    let mut c = {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/C", cmd]);
        c
    };
    #[cfg(not(windows))]
    let mut c = {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", cmd]);
        c
    };
    c.env("IDMX_FILE", path)
        .env("IDMX_URL", &job.url)
        .env("IDMX_NAME", path.file_name().unwrap_or_default())
        .env("IDMX_SIZE", job.size.unwrap_or(0).to_string())
        .env("IDMX_SHA256", sha256.unwrap_or(""))
        .current_dir(path.parent().unwrap_or(Path::new(".")))
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null());
    let out = tokio::time::timeout(Duration::from_secs(600), c.output())
        .await
        .map_err(|_| "command timed out after 10 minutes".to_string())?
        .map_err(|e| format!("couldn't start: {e}"))?;
    if out.status.success() {
        Ok(String::new())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: String = err.lines().rev().take(2).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" ⏎ ");
        Err(format!("`{}` exited with {}: {}", short(cmd), out.status.code().map_or("a signal".into(), |c| c.to_string()), tail))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn job(url: &str, size: u64) -> Job {
        Job {
            id: "1".into(),
            url: url.into(),
            final_url: None,
            file_name: "x".into(),
            dest_dir: ".".into(),
            path: None,
            status: JobStatus::Completed,
            size: Some(size),
            downloaded: size,
            speed: 0.0,
            eta: None,
            error: None,
            resumable: true,
            priority: 0,
            start_at: None,
            created_at: 0,
            started_at: None,
            completed_at: None,
            options: JobOptions::default(),
            networks: vec![],
            blocks_total: 0,
            blocks_done: 0,
            checksum: None,
            etag: None,
            last_modified: None,
            log: vec![],
            mime: None,
            kind: JobKind::Http,
            torrent: None,
            media: None,
            parent: None,
            peak_streams: 0,
            speed_history: vec![],
        }
    }

    #[test]
    fn matching() {
        let j = job("https://cdn.example.com/a/b.iso", 5 << 30);
        let m = RuleMatch { extensions: vec!["iso".into()], host: Some("example.com".into()), min_size: Some(1 << 30), ..Default::default() };
        assert!(rule_matches(&m, &j, "ubuntu.iso"));
        assert!(!rule_matches(&m, &j, "ubuntu.zip"));
        let m2 = RuleMatch { host: Some("other.org".into()), ..Default::default() };
        assert!(!rule_matches(&m2, &j, "ubuntu.iso"));
        let m3 = RuleMatch { name_regex: Some(r"^ubuntu-\d+".into()), ..Default::default() };
        assert!(rule_matches(&m3, &j, "Ubuntu-24.iso"));
        // An invalid regex never matches rather than matching everything.
        let bad = RuleMatch { name_regex: Some("(".into()), ..Default::default() };
        assert!(!rule_matches(&bad, &j, "anything"));
    }

    #[test]
    fn templates() {
        let j = job("https://dl.example.com/f.pdf", 1);
        let s = expand_template("/data/{host}/{ext}/{stem}", &j, "report.pdf");
        assert_eq!(s, "/data/dl.example.com/pdf/report");
    }

    #[tokio::test]
    async fn move_and_extract_and_run() {
        let dir = tempfile::tempdir().unwrap();
        // Build a zip containing a normal file.
        let zp = dir.path().join("pack.zip");
        {
            use std::io::Write;
            let f = std::fs::File::create(&zp).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            w.start_file("inner/hello.txt", opts).unwrap();
            w.write_all(b"hi").unwrap();
            w.finish().unwrap();
        }
        let j = job("https://h/pack.zip", 10);
        let rules = vec![PostRule {
            id: "r".into(),
            name: "unpack".into(),
            enabled: true,
            when: RuleMatch { extensions: vec!["zip".into()], ..Default::default() },
            actions: vec![
                PostAction::Extract { dir: None, delete_archive: false },
                PostAction::MoveTo { dir: dir.path().join("archive").to_string_lossy().into_owned() },
                #[cfg(unix)]
                PostAction::Run { command: "test -f \"$IDMX_FILE\" && echo ok > \"$IDMX_FILE.flag\"".into() },
            ],
        }];
        let out = apply(&rules, &j, zp.clone(), None, &|_| {}).await;
        assert!(dir.path().join("pack/inner/hello.txt").exists(), "{:?}", out.log);
        assert_eq!(out.path, dir.path().join("archive/pack.zip"));
        #[cfg(unix)]
        assert!(dir.path().join("archive/pack.zip.flag").exists(), "{:?}", out.log);
    }

    #[tokio::test]
    async fn failing_command_is_logged_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.bin");
        std::fs::write(&f, b"x").unwrap();
        let rules = vec![PostRule {
            id: "r".into(),
            name: "bad".into(),
            enabled: true,
            when: RuleMatch::default(),
            actions: vec![PostAction::Run { command: "exit 3".into() }],
        }];
        let out = apply(&rules, &job("https://h/a.bin", 1), f.clone(), None, &|_| {}).await;
        assert!(out.log.iter().any(|l| l.contains("exited with 3")), "{:?}", out.log);
        assert_eq!(out.path, f);
    }

    #[test]
    fn zip_slip_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let zp = dir.path().join("evil.zip");
        {
            use std::io::Write;
            let f = std::fs::File::create(&zp).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            w.start_file("../escaped.txt", opts).unwrap();
            w.write_all(b"pwn").unwrap();
            w.finish().unwrap();
        }
        let out = dir.path().join("out");
        let n = extract(&zp, &out, false).unwrap();
        assert_eq!(n, 0);
        assert!(!dir.path().join("escaped.txt").exists());
    }
}
