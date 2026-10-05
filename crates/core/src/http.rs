//! Everything that touches HTTP semantics: building a client pinned to one network,
//! shaping requests, and probing a URL for its size and range support.

use crate::error::{Error, Result};
use crate::types::{JobOptions, ProbeInfo};
use crate::util::sanitize_filename;
use percent_encoding::percent_decode_str;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT_ENCODING, AUTHORIZATION, CONTENT_RANGE, COOKIE, RANGE, REFERER, USER_AGENT};
use reqwest::{Client, Proxy, RequestBuilder, Response};
use std::net::IpAddr;
use std::time::{Duration, Instant};
use url::Url;

pub const DEFAULT_UA: &str = concat!("IDMX/", env!("CARGO_PKG_VERSION"), " (+multi-network)");

#[derive(Clone, Debug, Default)]
pub struct ClientSpec {
    /// Source address to bind.
    pub local: Option<IpAddr>,
    /// Device to force egress through (`IP_BOUND_IF` / `SO_BINDTODEVICE`). Where the OS lets
    /// us, this is what actually keeps a socket on one network instead of the default route.
    pub device: Option<String>,
    pub proxy: Option<String>,
    /// Pin the target host to these addresses (already filtered to one IP family), so the
    /// connection can only be made over the family this network can actually use.
    pub pin: Option<(String, Vec<std::net::SocketAddr>)>,
}

pub fn build_client(spec: &ClientSpec) -> Result<Client> {
    let mut b = Client::builder()
        .http1_only()
        .redirect(reqwest::redirect::Policy::limited(10))
        .connect_timeout(Duration::from_secs(15))
        .pool_max_idle_per_host(64)
        .pool_idle_timeout(Duration::from_secs(45))
        .tcp_nodelay(true)
        .tcp_keepalive(Duration::from_secs(30));
    if let Some(ip) = spec.local {
        b = b.local_address(ip);
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    if let Some(dev) = &spec.device {
        b = b.interface(dev);
    }
    if let Some((host, addrs)) = &spec.pin {
        if spec.proxy.as_deref().is_none_or(str::is_empty) && !addrs.is_empty() {
            b = b.resolve_to_addrs(host, addrs);
        }
    }
    if let Some(p) = spec.proxy.as_deref().filter(|p| !p.is_empty()) {
        b = b.proxy(Proxy::all(p).map_err(|e| Error::msg(format!("The proxy address isn't valid ({e}). Use something like http://host:8080 or socks5://host:1080.")))?);
    }
    Ok(b.build()?)
}

/// Headers common to every request of a job.
pub fn base_headers(opts: &JobOptions, global_ua: Option<&str>) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    let ua = opts.user_agent.as_deref().or(global_ua).unwrap_or(DEFAULT_UA);
    if let Ok(v) = HeaderValue::from_str(ua) {
        h.insert(USER_AGENT, v);
    }
    if let Some(r) = &opts.referer {
        if let Ok(v) = HeaderValue::from_str(r) {
            h.insert(REFERER, v);
        }
    }
    if let Some(c) = &opts.cookies {
        if let Ok(v) = HeaderValue::from_str(c) {
            h.insert(COOKIE, v);
        }
    }
    if let Some(t) = &opts.bearer {
        if let Ok(v) = HeaderValue::from_str(&format!("Bearer {t}")) {
            h.insert(AUTHORIZATION, v);
        }
    } else if let Some(a) = &opts.auth {
        use base64::Engine;
        let tok = base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", a.user, a.password));
        if let Ok(v) = HeaderValue::from_str(&format!("Basic {tok}")) {
            h.insert(AUTHORIZATION, v);
        }
    }
    for (k, v) in &opts.headers {
        if let (Ok(k), Ok(v)) = (HeaderName::from_bytes(k.trim().as_bytes()), HeaderValue::from_str(v.trim())) {
            h.insert(k, v);
        }
    }
    h
}

pub fn ranged(client: &Client, url: &str, headers: &HeaderMap, start: u64, end: Option<u64>) -> RequestBuilder {
    let range = match end {
        Some(e) => format!("bytes={start}-{e}"),
        None => format!("bytes={start}-"),
    };
    client.get(url).headers(headers.clone()).header(RANGE, range)
}

pub fn header_str<'a>(r: &'a Response, name: impl reqwest::header::AsHeaderName) -> Option<&'a str> {
    r.headers().get(name).and_then(|v| v.to_str().ok())
}

/// `bytes 0-0/12345` → `(0, 0, Some(12345))`; `bytes */0` → `(0, 0, Some(0))`.
pub fn parse_content_range(v: &str) -> Option<(u64, u64, Option<u64>)> {
    let v = v.trim().strip_prefix("bytes")?.trim();
    let (range, total) = v.split_once('/')?;
    let total = total.trim().parse::<u64>().ok();
    if range.trim() == "*" {
        return Some((0, 0, total));
    }
    let (a, b) = range.split_once('-')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?, total))
}

pub fn filename_from_disposition(v: &str) -> Option<String> {
    // RFC 5987 `filename*=UTF-8''%e2%82%ac.txt` wins over the plain form.
    for part in v.split(';').map(str::trim) {
        let lower = part.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("filename*=") {
            let raw = &part[part.len() - rest.len()..];
            let enc = raw.splitn(3, '\'').nth(2).unwrap_or(raw);
            let dec = percent_decode_str(enc.trim_matches('"')).decode_utf8_lossy().into_owned();
            if !dec.is_empty() {
                return Some(dec);
            }
        }
    }
    for part in v.split(';').map(str::trim) {
        let lower = part.to_ascii_lowercase();
        if lower.starts_with("filename=") {
            let raw = part["filename=".len()..].trim().trim_matches('"');
            if !raw.is_empty() {
                return Some(raw.to_string());
            }
        }
    }
    None
}

pub fn filename_from_url(u: &Url) -> String {
    let seg = u.path_segments().and_then(|mut s| s.next_back()).unwrap_or("");
    let dec = percent_decode_str(seg).decode_utf8_lossy().into_owned();
    if dec.is_empty() {
        u.host_str().map(|h| format!("{h}.html")).unwrap_or_else(|| "download".into())
    } else {
        dec
    }
}

pub fn parse_url(s: &str) -> Result<Url> {
    let t = s.trim();
    let u = Url::parse(t).map_err(|_| {
        if t.is_empty() {
            Error::BadUrl("The address is empty.".into())
        } else if !t.contains("://") {
            Error::BadUrl(format!("“{t}” doesn't look like a web address. Links start with http:// or https://."))
        } else {
            Error::BadUrl(format!("“{t}” isn't a valid web address — check it for typos or missing parts."))
        }
    })?;
    match u.scheme() {
        "http" | "https" => Ok(u),
        other => Err(Error::BadUrl(format!("IDMX downloads http:// and https:// links; “{other}://” isn't supported."))),
    }
}

/// Ask for one byte. A `206` proves range requests work end to end — more reliable than
/// trusting `Accept-Ranges` on a HEAD, which CDNs frequently get wrong.
pub async fn probe(client: &Client, url: &str, headers: &HeaderMap) -> Result<(ProbeInfo, Response)> {
    let u = parse_url(url)?;
    let t0 = Instant::now();
    let resp = ranged(client, u.as_str(), headers, 0, Some(0)).send().await?;
    let ttfb = t0.elapsed().as_millis() as u64;
    let status = resp.status();
    let final_url = resp.url().to_string();

    let (size, ranges) = match status.as_u16() {
        206 => {
            let total = header_str(&resp, CONTENT_RANGE).and_then(parse_content_range).and_then(|c| c.2);
            (total, true)
        }
        // Empty file: range not satisfiable, `bytes */0`.
        416 => (header_str(&resp, CONTENT_RANGE).and_then(parse_content_range).and_then(|c| c.2).or(Some(0)), true),
        200 => (resp.content_length(), false),
        c => return Err(Error::Http(c)),
    };
    let name = header_str(&resp, "content-disposition")
        .and_then(filename_from_disposition)
        .unwrap_or_else(|| filename_from_url(resp.url()));
    let info = ProbeInfo {
        url: url.to_string(),
        final_url,
        file_name: sanitize_filename(&name),
        size,
        ranges,
        mime: header_str(&resp, "content-type").map(|s| s.split(';').next().unwrap_or(s).trim().to_string()),
        etag: header_str(&resp, "etag").map(str::to_string),
        last_modified: header_str(&resp, "last-modified").map(str::to_string),
        status: status.as_u16(),
        ttfb_ms: ttfb,
    };
    Ok((info, resp))
}

/// Checksums a server publishes in its headers: `(algo, hex)`.
pub fn header_checksum(resp: &Response) -> Option<(String, String)> {
    use base64::Engine;
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.decode(s.trim()).ok().map(hex::encode);
    if let Some(d) = header_str(resp, "digest") {
        for item in d.split(',') {
            let (alg, val) = item.trim().split_once('=')?;
            match alg.to_ascii_lowercase().as_str() {
                "sha-256" => return b64(val).map(|h| ("sha256".into(), h)),
                "sha-1" => return b64(val).map(|h| ("sha1".into(), h)),
                "md5" => return b64(val).map(|h| ("md5".into(), h)),
                _ => {}
            }
        }
    }
    if let Some(v) = header_str(resp, "x-amz-checksum-sha256") {
        return b64(v).map(|h| ("sha256".into(), h));
    }
    if let Some(v) = header_str(resp, "content-md5") {
        return b64(v).map(|h| ("md5".into(), h));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range() {
        assert_eq!(parse_content_range("bytes 0-0/12345"), Some((0, 0, Some(12345))));
        assert_eq!(parse_content_range("bytes 5-9/*"), Some((5, 9, None)));
        assert_eq!(parse_content_range("bytes */0"), Some((0, 0, Some(0))));
        assert_eq!(parse_content_range("items 0-1/2"), None);
    }

    #[test]
    fn disposition() {
        assert_eq!(filename_from_disposition(r#"attachment; filename="a b.zip""#).as_deref(), Some("a b.zip"));
        assert_eq!(filename_from_disposition("attachment; filename=x.iso").as_deref(), Some("x.iso"));
        assert_eq!(
            filename_from_disposition("attachment; filename=\"fallback.txt\"; filename*=UTF-8''%E2%82%AC%20rates.txt").as_deref(),
            Some("€ rates.txt")
        );
        assert_eq!(filename_from_disposition("inline"), None);
    }

    #[test]
    fn url_names() {
        let u = Url::parse("https://h/a/b/My%20File.tar.gz?x=1").unwrap();
        assert_eq!(filename_from_url(&u), "My File.tar.gz");
    }
}

/// Build a `Cookie:` header for `url` from a Netscape-format `cookies.txt` (what browser
/// extensions like "Get cookies.txt" export, and what `curl -b` reads).
pub fn cookies_for(url: &str, cookies_txt: &str) -> String {
    let Ok(u) = Url::parse(url) else { return String::new() };
    let host = u.host_str().unwrap_or("").to_ascii_lowercase();
    let secure = u.scheme() == "https";
    let now = chrono::Utc::now().timestamp();
    let mut out: Vec<String> = Vec::new();
    for line in cookies_txt.lines() {
        // `#HttpOnly_` prefixes a real cookie; any other `#` line is a comment.
        let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 7 {
            continue;
        }
        let domain = f[0].trim_start_matches('.').to_ascii_lowercase();
        let domain_ok = host == domain || host.ends_with(&format!(".{domain}"));
        let path_ok = u.path().starts_with(f[2]);
        let secure_ok = !f[3].eq_ignore_ascii_case("TRUE") || secure;
        let expiry = f[4].parse::<i64>().unwrap_or(0);
        if domain_ok && path_ok && secure_ok && (expiry == 0 || expiry > now) {
            out.push(format!("{}={}", f[5], f[6].trim()));
        }
    }
    out.join("; ")
}

#[cfg(test)]
mod cookie_tests {
    use super::*;

    #[test]
    fn picks_matching_cookies_only() {
        let txt = "# Netscape HTTP Cookie File\n\
.example.com\tTRUE\t/\tFALSE\t0\tsession\tabc\n\
.example.com\tTRUE\t/private\tFALSE\t0\tpriv\tyes\n\
other.org\tFALSE\t/\tFALSE\t0\tnope\tx\n\
#HttpOnly_.example.com\tTRUE\t/\tTRUE\t0\tsecure\ts1\n\
.example.com\tTRUE\t/\tFALSE\t1\texpired\told\n";
        assert_eq!(cookies_for("https://dl.example.com/file.zip", txt), "session=abc; secure=s1");
        assert_eq!(cookies_for("http://example.com/file.zip", txt), "session=abc");
        assert_eq!(cookies_for("https://example.com/private/x", txt), "session=abc; priv=yes; secure=s1");
    }
}
