//! The live terminal view. `frame()` is a pure function from state to lines, so it can be tested
//! without a terminal; `Painter` is the only thing that touches the cursor.

use crate::fmt;
use idmx_core::types::*;
use std::collections::{BTreeMap, HashMap};
use std::io::Write;

pub const RESET: &str = "\x1b[0m";
pub const DIM: &str = "\x1b[2m";
pub const BOLD: &str = "\x1b[1m";

pub fn rgb(r: u8, g: u8, b: u8) -> String {
    format!("\x1b[38;2;{r};{g};{b}m")
}

pub fn net_color(kind: IfKind) -> (u8, u8, u8) {
    match kind {
        IfKind::Wifi => (64, 200, 255),
        IfKind::Ethernet => (52, 211, 153),
        IfKind::Cellular => (232, 121, 249),
        IfKind::Tether => (251, 191, 36),
        IfKind::Vpn => (129, 140, 248),
        _ => (156, 163, 175),
    }
}

fn paint(c: (u8, u8, u8), s: &str) -> String {
    format!("{}{}{}", rgb(c.0, c.1, c.2), s, RESET)
}

fn status_label(s: JobStatus) -> (&'static str, (u8, u8, u8)) {
    match s {
        JobStatus::Queued => ("queued", (156, 163, 175)),
        JobStatus::Probing => ("probing…", (251, 191, 36)),
        JobStatus::Downloading => ("downloading", (64, 200, 255)),
        JobStatus::Paused => ("paused", (251, 191, 36)),
        JobStatus::Verifying => ("verifying…", (129, 140, 248)),
        JobStatus::Processing => ("processing…", (129, 140, 248)),
        JobStatus::Seeding => ("seeding", (52, 211, 153)),
        JobStatus::Completed => ("done", (52, 211, 153)),
        JobStatus::Failed => ("failed", (248, 113, 113)),
        JobStatus::Cancelled => ("cancelled", (156, 163, 175)),
    }
}

#[derive(Default)]
pub struct View {
    pub jobs: BTreeMap<String, Job>,
    pub maps: HashMap<String, BlockMap>,
}

/// A bar split between networks in proportion to what each delivered.
pub fn stacked_bar(width: usize, frac: f64, nets: &[NetStat]) -> String {
    let filled = ((frac.clamp(0.0, 1.0)) * width as f64).round() as usize;
    let total: u64 = nets.iter().map(|n| n.bytes).sum();
    let mut out = String::new();
    let mut used = 0;
    if total == 0 {
        out.push_str(&paint((156, 163, 175), &"█".repeat(filled)));
        used = filled;
    } else {
        let mut acc = 0u64;
        for n in nets.iter().filter(|n| n.bytes > 0) {
            acc += n.bytes;
            let upto = (acc as f64 / total as f64 * filled as f64).round() as usize;
            if upto > used {
                out.push_str(&paint(net_color(n.kind), &"█".repeat(upto - used)));
                used = upto;
            }
        }
    }
    out.push_str(&format!("{DIM}{}{RESET}", "░".repeat(width.saturating_sub(used))));
    out
}

pub fn map_line(width: usize, map: &BlockMap, nets: &[NetStat]) -> String {
    let n = map.state.len();
    if n == 0 || width == 0 {
        return String::new();
    }
    let color_of = |owner: u8| -> (u8, u8, u8) {
        map.networks
            .get(owner as usize)
            .and_then(|id| nets.iter().find(|s| &s.id == id))
            .map(|s| net_color(s.kind))
            .unwrap_or((156, 163, 175))
    };
    let mut out = String::new();
    for c in 0..width {
        let (a, b) = (c * n / width, (((c + 1) * n) / width).max(c * n / width + 1).min(n));
        let (mut fill, mut active) = (0.0f32, false);
        let mut votes: HashMap<u8, u32> = HashMap::new();
        for i in a..b {
            fill += map.fill[i];
            active |= map.state[i] == 1;
            if map.owner[i] != 255 {
                *votes.entry(map.owner[i]).or_default() += 1;
            }
        }
        fill /= (b - a) as f32;
        let owner = votes.into_iter().max_by_key(|(_, v)| *v).map(|(o, _)| o);
        let col = owner.map(color_of).unwrap_or((156, 163, 175));
        out.push_str(&if fill >= 0.999 {
            paint(col, "█")
        } else if active || fill > 0.0 {
            paint(col, "▓")
        } else {
            format!("{DIM}░{RESET}")
        });
    }
    out
}

/// Visible width of a string, ignoring ANSI escapes.
#[cfg(test)]
pub fn visible_len(s: &str) -> usize {
    let mut n = 0;
    let mut esc = false;
    for ch in s.chars() {
        match (esc, ch) {
            (false, '\x1b') => esc = true,
            (true, 'm') => esc = false,
            (true, _) => {}
            (false, _) => n += 1,
        }
    }
    n
}

pub fn frame(view: &View, width: usize, only: &[String]) -> Vec<String> {
    let mut lines = Vec::new();
    let bar_w = width.saturating_sub(34).clamp(10, 56);
    for (id, j) in view.jobs.iter().filter(|(id, _)| only.is_empty() || only.contains(id)) {
        let (label, lc) = status_label(j.status);
        let size = j.size.map(fmt::bytes).unwrap_or_else(|| "?".into());
        let name = fmt::clip(&j.file_name, width.saturating_sub(34).max(12));
        lines.push(format!("{BOLD}{name}{RESET}  {DIM}{size}{RESET}  {}  {DIM}{id}{RESET}", paint(lc, label)));

        let frac = match (j.size, j.downloaded) {
            (Some(s), d) if s > 0 => d as f64 / s as f64,
            _ if j.status == JobStatus::Completed => 1.0,
            _ => 0.0,
        };
        let eta = if j.status == JobStatus::Downloading { fmt::eta(j.eta) } else { String::new() };
        lines.push(format!(
            "  {}  {}  {}{:>12}{RESET}  {DIM}{}{RESET}",
            stacked_bar(bar_w, frac, &j.networks),
            fmt::pct(j.downloaded, j.size),
            BOLD,
            if j.status == JobStatus::Downloading { fmt::rate(j.speed) } else { String::new() },
            eta
        ));

        if matches!(j.status, JobStatus::Downloading | JobStatus::Paused | JobStatus::Probing) || (j.status == JobStatus::Completed && !j.networks.is_empty()) {
            let max_speed = j.networks.iter().map(|n| n.speed).fold(1.0, f64::max);
            let total: u64 = j.networks.iter().map(|n| n.bytes).sum();
            for (i, n) in j.networks.iter().enumerate() {
                let c = net_color(n.kind);
                let cells = ((n.speed / max_speed) * 10.0).round() as usize;
                let branch = if i + 1 == j.networks.len() { "└" } else { "├" };
                let why = |s: &str| match &n.error {
                    Some(e) => format!("{s}: {}", fmt::clip(e, 40)),
                    None => s.to_string(),
                };
                let state = match n.state {
                    NetState::Unreachable => format!(" {}", paint((248, 113, 113), &why("unreachable"))),
                    NetState::Failed => format!(" {}", paint((248, 113, 113), &why("refused"))),
                    NetState::Capped => format!(" {}", paint((251, 191, 36), "capped")),
                    NetState::Offline => format!(" {}", paint((248, 113, 113), "offline")),
                    NetState::Disabled => format!(" {}", paint((156, 163, 175), "off")),
                    _ => String::new(),
                };
                let share = if total > 0 { format!("{:>3.0}%", n.bytes as f64 / total as f64 * 100.0) } else { "    ".into() };
                let tail = if j.status == JobStatus::Completed {
                    format!("{share}  {}", fmt::bytes(n.bytes))
                } else {
                    format!("{:>11}  ×{}/{}  {:>4.0} ms", fmt::rate(n.speed), n.active, n.streams, n.latency_ms)
                };
                lines.push(format!(
                    "  {DIM}{branch}{RESET} {} {:<14} {}{}{DIM}{}{RESET}  {tail}{state}",
                    paint(c, "●"),
                    fmt::clip(&n.label, 14),
                    rgb(c.0, c.1, c.2),
                    "▮".repeat(cells.min(10)),
                    "▯".repeat(10 - cells.min(10)),
                ));
            }
        }
        if j.status == JobStatus::Downloading {
            if let Some(m) = view.maps.get(id) {
                lines.push(format!("  {DIM}map{RESET} {}", map_line(width.saturating_sub(8).min(72), m, &j.networks)));
            }
        }
        if let Some(e) = &j.error {
            lines.push(format!("  {}", paint((248, 113, 113), &format!("✗ {e}"))));
        }
        lines.push(String::new());
    }
    lines
}

/// Redraws a block of lines in place.
pub struct Painter {
    drawn: usize,
}

impl Painter {
    pub fn new() -> Self {
        print!("\x1b[?25l"); // hide cursor
        Painter { drawn: 0 }
    }

    pub fn draw(&mut self, lines: &[String]) {
        let mut out = String::new();
        if self.drawn > 0 {
            out.push_str(&format!("\x1b[{}A", self.drawn));
        }
        for l in lines {
            out.push_str(&format!("\r\x1b[2K{l}\n"));
        }
        // Erase leftovers if the block shrank.
        for _ in lines.len()..self.drawn {
            out.push_str("\r\x1b[2K\n");
        }
        if self.drawn > lines.len() {
            out.push_str(&format!("\x1b[{}A", self.drawn - lines.len()));
        }
        self.drawn = lines.len();
        let mut so = std::io::stdout().lock();
        let _ = so.write_all(out.as_bytes());
        let _ = so.flush();
    }
}

impl Drop for Painter {
    fn drop(&mut self) {
        print!("\x1b[?25h"); // show cursor
        let _ = std::io::stdout().flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(id: &str, kind: IfKind, bytes: u64, speed: f64) -> NetStat {
        NetStat { id: id.into(), label: id.into(), kind, bytes, speed, streams: 8, active: 8, latency_ms: 12.0, health: 100, errors: 0, state: NetState::Active, error: None, enabled: true }
    }

    fn job() -> Job {
        serde_json::from_value(serde_json::json!({
            "id": "abc12345", "url": "http://x/y", "finalUrl": null, "fileName": "ubuntu-24.04-desktop-amd64.iso",
            "destDir": "/tmp", "path": null, "status": "downloading", "size": 1000, "downloaded": 500, "speed": 1e6,
            "eta": 12, "error": null, "resumable": true, "priority": 0, "startAt": null, "createdAt": 0,
            "startedAt": null, "completedAt": null, "options": {}, "networks": [], "blocksTotal": 0, "blocksDone": 0,
            "checksum": null, "etag": null, "lastModified": null, "log": [], "mime": null
        }))
        .unwrap()
    }

    #[test]
    fn bar_has_exact_width_and_colours_by_network() {
        let nets = vec![net("a", IfKind::Wifi, 300, 1.0), net("b", IfKind::Tether, 100, 1.0)];
        let b = stacked_bar(40, 0.5, &nets);
        assert_eq!(visible_len(&b), 40);
        assert!(b.contains(&rgb(64, 200, 255)));
        assert!(b.contains(&rgb(251, 191, 36)));
        assert_eq!(visible_len(&stacked_bar(40, 0.0, &[])), 40);
        assert_eq!(visible_len(&stacked_bar(40, 1.0, &nets)), 40);
    }

    #[test]
    fn frame_shows_everything_that_matters() {
        let mut j = job();
        j.networks = vec![net("Wi-Fi", IfKind::Wifi, 300, 2e6), net("iPhone", IfKind::Tether, 200, 1e6)];
        let mut v = View::default();
        v.jobs.insert(j.id.clone(), j);
        let text: String = frame(&v, 100, &[]).join("\n");
        for needle in ["ubuntu", ".iso", "50.0%", "Wi-Fi", "iPhone", "×8/8", "downloading"] {
            assert!(text.contains(needle), "missing {needle} in:\n{text}");
        }
    }

    #[test]
    fn map_line_has_exact_width() {
        let m = BlockMap {
            block_size: 1,
            networks: vec!["a".into()],
            state: vec![2, 2, 1, 0, 0, 0],
            owner: vec![0, 0, 0, 255, 255, 255],
            fill: vec![1.0, 1.0, 0.5, 0.0, 0.0, 0.0],
            split: vec![],
        };
        assert_eq!(visible_len(&map_line(30, &m, &[net("a", IfKind::Wifi, 1, 1.0)])), 30);
        assert_eq!(visible_len(&map_line(4, &m, &[])), 4);
    }
}
