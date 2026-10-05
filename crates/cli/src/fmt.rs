//! Small formatting helpers shared by the CLI's views.

pub fn bytes(n: u64) -> String {
    idmx_core::util::human_bytes(n)
}

pub fn rate(bps: f64) -> String {
    if bps < 1.0 {
        return "—".into();
    }
    format!("{}/s", bytes(bps as u64))
}

pub fn eta(secs: Option<u64>) -> String {
    match secs {
        None => "—:—".into(),
        Some(s) if s >= 86_400 => format!("{}d {}h", s / 86_400, (s % 86_400) / 3600),
        Some(s) if s >= 3600 => format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60),
        Some(s) => format!("{}:{:02}", s / 60, s % 60),
    }
}

pub fn pct(done: u64, total: Option<u64>) -> String {
    match total {
        Some(t) if t > 0 => format!("{:5.1}%", done as f64 / t as f64 * 100.0),
        _ => "  …  ".into(),
    }
}

pub fn clip(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    // Keep the extension visible: "very-long-na….iso".
    let ext_start = s.rfind('.').filter(|i| s.len() - i <= 8).unwrap_or(s.len());
    let ext: String = s[ext_start..].to_string();
    let head: String = s.chars().take(keep.saturating_sub(ext.chars().count())).collect();
    format!("{head}…{ext}")
}

/// Parse `2026-10-05 01:30`, `01:30` (next occurrence), or `+45m` / `+2h` / `+1d`.
pub fn parse_when(s: &str) -> Option<i64> {
    use chrono::{Duration, Local, NaiveDateTime, NaiveTime, TimeZone};
    let s = s.trim();
    let now = Local::now();
    if let Some(rest) = s.strip_prefix('+') {
        let (n, unit) = rest.split_at(rest.len().checked_sub(1)?);
        let n: i64 = n.parse().ok()?;
        let d = match unit {
            "m" => Duration::minutes(n),
            "h" => Duration::hours(n),
            "d" => Duration::days(n),
            _ => return None,
        };
        return Some((now + d).timestamp());
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M") {
        return Local.from_local_datetime(&dt).single().map(|d| d.timestamp());
    }
    if let Ok(t) = NaiveTime::parse_from_str(s, "%H:%M") {
        let mut when = Local.from_local_datetime(&now.date_naive().and_time(t)).single()?;
        if when <= now {
            when += Duration::days(1);
        }
        return Some(when.timestamp());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(eta(Some(75)), "1:15");
        assert_eq!(eta(Some(3725)), "1:02:05");
        assert_eq!(eta(None), "—:—");
        assert_eq!(rate(0.0), "—");
        assert_eq!(pct(50, Some(200)), " 25.0%");
    }

    #[test]
    fn clips_keeping_the_extension() {
        let c = clip("a-really-long-file-name-here.iso", 16);
        assert_eq!(c.chars().count(), 16);
        assert!(c.ends_with(".iso"));
        assert_eq!(clip("short.txt", 16), "short.txt");
    }

    #[test]
    fn when() {
        let now = chrono::Local::now().timestamp();
        let t = parse_when("+2h").unwrap();
        assert!((t - now - 7200).abs() < 3);
        assert!(parse_when("23:59").unwrap() > now);
        assert!(parse_when("2030-01-01 00:00").unwrap() > now);
        assert!(parse_when("garbage").is_none());
    }
}
