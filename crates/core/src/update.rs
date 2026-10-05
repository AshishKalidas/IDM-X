//! Is there a newer release? Asks GitHub's "latest release" endpoint, politely and rarely.

use crate::error::{Error, Result};
use crate::types::UpdateInfo;
use serde::Deserialize;
use std::cmp::Ordering;
use std::time::Duration;

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
}

/// `v1.2.3` / `1.2.3-rc.4` → numbers and whether it's a pre-release.
pub fn parse_version(s: &str) -> Option<([u64; 3], bool)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let (core, pre) = match s.split_once('-') {
        Some((c, _)) => (c, true),
        None => (s, false),
    };
    let core = core.split('+').next()?;
    let mut it = core.split('.');
    let mut n = [0u64; 3];
    for slot in &mut n {
        *slot = it.next().map(|p| p.parse().ok()).unwrap_or(Some(0))?;
    }
    Some((n, pre))
}

/// Is `candidate` a newer release than `current`? A pre-release of the same number is older.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let (Some((c, cpre)), Some((cur, curpre))) = (parse_version(candidate), parse_version(current)) else { return false };
    match c.cmp(&cur) {
        Ordering::Greater => true,
        Ordering::Equal => curpre && !cpre,
        Ordering::Less => false,
    }
}

/// The URL to ask, from `owner/name` — or a full URL (used by tests and mirrors).
pub fn feed_url(repo: &str) -> Option<String> {
    let r = repo.trim();
    if r.is_empty() {
        return None;
    }
    if r.starts_with("http://") || r.starts_with("https://") {
        return Some(r.to_string());
    }
    let ok = r.split('/').count() == 2 && r.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'));
    ok.then(|| format!("https://api.github.com/repos/{r}/releases/latest"))
}

/// The newest published release, if it is newer than `current`.
pub async fn check(client: &reqwest::Client, repo: &str, current: &str) -> Result<Option<UpdateInfo>> {
    let url = feed_url(repo).ok_or_else(|| Error::msg("set the update source as owner/repo"))?;
    let resp = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", concat!("IDMX/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(10))
        .send()
        .await?;
    if resp.status().as_u16() == 404 {
        return Ok(None); // no releases yet
    }
    if !resp.status().is_success() {
        return Err(Error::Http(resp.status().as_u16()));
    }
    let r: Release = resp.json().await?;
    if r.draft || r.prerelease || !is_newer(&r.tag_name, current) {
        return Ok(None);
    }
    Ok(Some(UpdateInfo { version: r.tag_name.trim_start_matches(['v', 'V']).to_string(), url: r.html_url, dismissed: false }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_like_people_expect() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.1.10", "0.1.9"), "numeric, not alphabetical");
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        // A release candidate is older than its final release, but newer than the one before.
        assert!(is_newer("1.0.0", "1.0.0-rc.1"));
        assert!(!is_newer("1.0.0-rc.2", "1.0.0"));
        assert!(is_newer("1.0.0-rc.1", "0.9.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn feed_urls() {
        assert_eq!(feed_url("acme/idmx").unwrap(), "https://api.github.com/repos/acme/idmx/releases/latest");
        assert!(feed_url("").is_none());
        assert!(feed_url("a b/c").is_none());
        assert!(feed_url("../../etc").is_none());
        assert_eq!(feed_url("http://localhost:1/x").unwrap(), "http://localhost:1/x");
    }
}
