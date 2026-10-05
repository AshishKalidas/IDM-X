use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

/// What the user sees when there is no network at all. Jobs that fail with exactly this are put
/// back in the queue by themselves when a network returns.
pub const NO_NETWORK_MSG: &str = "No network connection. Connect to Wi-Fi or plug in a cable and IDMX will carry on by itself.";

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    Msg(String),
    #[error("{0}")]
    BadUrl(String),
    #[error("{}", http_message(*.0))]
    Http(u16),
    #[error("The file on the server changed since this download started.")]
    RemoteChanged,
    #[error("Not enough space on the disk: this needs {} and only {} is free.", crate::util::human_bytes(*.need), crate::util::human_bytes(*.have))]
    DiskFull { need: u64, have: u64 },
    #[error("{NO_NETWORK_MSG}")]
    NoNetwork,
    #[error("The file arrived damaged: its checksum is {actual} but the server published {expected}. It was deleted so a bad copy can't be used — retry to download it again.")]
    Checksum { expected: String, actual: String },
    #[error("Not found.")]
    NotFound,
    #[error("{}", io_message(.0))]
    Io(#[from] std::io::Error),
    #[error("{}", net_message(.0))]
    Net(#[from] reqwest::Error),
    #[error("Couldn't read that data: {0}")]
    Json(#[from] serde_json::Error),
}

impl Error {
    pub fn msg(s: impl Into<String>) -> Self {
        Error::Msg(s.into())
    }

    /// Worth trying again later: the network or the server had a bad moment.
    pub fn is_transient(&self) -> bool {
        match self {
            Error::Net(_) | Error::Io(_) => true,
            Error::Http(c) => matches!(*c, 408 | 425 | 429 | 500 | 502 | 503 | 504 | 520..=524),
            _ => false,
        }
    }
}

/// What a status code means to a person, and what to do about it.
pub fn http_message(code: u16) -> String {
    match code {
        400 => "The server didn't understand the request (400). Check the link is complete.".into(),
        401 => "The server wants a login for this file (401). Add a username and password, a token or your cookies under Advanced.".into(),
        402 => "The server says this file needs payment (402).".into(),
        403 => "The server won't let IDMX download this (403). The link may have expired, or it needs you to be logged in — try adding your cookies, or copy the link again from the page.".into(),
        404 => "The file isn't there (404). The link may be wrong or the file may have been removed.".into(),
        405 => "The server doesn't allow downloading from this address (405).".into(),
        406 | 415 => format!("The server refused to send this file in a form IDMX accepts ({code})."),
        408 => "The server gave up waiting (408). Trying again usually works.".into(),
        410 => "The file has been removed from the server for good (410).".into(),
        416 => "The server rejected the byte range IDMX asked for (416) — the file probably changed.".into(),
        429 => "The server says IDMX is making too many requests (429). It will slow down and retry.".into(),
        451 => "This file is blocked where you are (451).".into(),
        500 => "The server hit an error (500). It's their problem, not yours — try again later.".into(),
        502 | 504 => format!("The server's gateway isn't responding ({code}). It's usually temporary — try again in a bit."),
        503 => "The server is overloaded or down for maintenance (503). IDMX will keep trying.".into(),
        520..=524 => format!("The site's CDN couldn't reach its server ({code}). It's usually temporary."),
        c if (500..600).contains(&c) => format!("The server had an internal error ({c}). Try again later."),
        c if (400..500).contains(&c) => format!("The server refused the request ({c})."),
        c => format!("The server answered something unexpected ({c})."),
    }
}

/// Dig the underlying cause out of a network error — the useful part is usually a few sources
/// down, behind "error sending request".
fn chain(e: &reqwest::Error) -> String {
    let mut out = String::new();
    let mut cur: Option<&dyn std::error::Error> = Some(e);
    while let Some(c) = cur {
        out.push_str(&c.to_string().to_lowercase());
        out.push(' ');
        cur = c.source();
    }
    out
}

pub fn net_message(e: &reqwest::Error) -> String {
    let c = chain(e);
    let host = e.url().and_then(|u| u.host_str()).unwrap_or("the server");
    if e.is_timeout() {
        return format!("{host} took too long to answer. Check your connection, or try again later.");
    }
    if e.is_redirect() || c.contains("too many redirects") {
        return format!("{host} keeps redirecting in a loop, so the real file can't be reached.");
    }
    if c.contains("dns error") || c.contains("failed to lookup") || c.contains("name or service not known") || c.contains("nodename nor servname") || c.contains("no such host") || c.contains("temporary failure in name resolution") {
        return format!("Couldn't find {host}. Check the address for typos, and that you're online.");
    }
    if c.contains("certificate") || c.contains("handshake") || c.contains("tls") || c.contains("ssl") {
        let why = if c.contains("expired") {
            "its security certificate has expired"
        } else if c.contains("self signed") || c.contains("self-signed") || c.contains("unknown issuer") || c.contains("unknownissuer") {
            "its security certificate isn't from a trusted authority"
        } else if c.contains("not valid for") || c.contains("notvalidforname") || c.contains("hostname") {
            "its certificate is for a different site"
        } else {
            "a secure connection couldn't be set up"
        };
        return format!("Can't safely connect to {host}: {why}. IDMX won't download over a connection it can't trust.");
    }
    if c.contains("connection refused") {
        return format!("{host} refused the connection. The server may be down, or the port in the address may be wrong.");
    }
    if c.contains("network is unreachable") || c.contains("no route to host") || c.contains("host is unreachable") {
        return "That network can't reach the internet right now. Check the cable or Wi-Fi, or switch this network off.".into();
    }
    if c.contains("can't assign requested address") || c.contains("cannot assign requested address") || c.contains("address not available") {
        return "This network's address changed or went away. IDMX will reconnect when it's back.".into();
    }
    if c.contains("connection reset") || c.contains("broken pipe") || c.contains("connection closed") || c.contains("unexpected eof") || c.contains("incomplete message") {
        return format!("The connection to {host} was cut off. IDMX will retry.");
    }
    if e.is_connect() {
        return format!("Couldn't connect to {host}. Check your connection and the address.");
    }
    if e.is_body() || e.is_decode() {
        return format!("The data from {host} stopped arriving properly. IDMX will retry.");
    }
    format!("A network problem occurred talking to {host}.")
}

pub fn io_message(e: &std::io::Error) -> String {
    use std::io::ErrorKind::*;
    match e.kind() {
        PermissionDenied => "IDMX isn't allowed to write to that folder. Pick a different folder, or change its permissions.".into(),
        NotFound => "A folder or file IDMX needed isn't there any more. Check that the destination folder still exists (is the drive plugged in?).".into(),
        StorageFull => "The disk is full. Free up some space and resume.".into(),
        ReadOnlyFilesystem => "That location is read-only, so nothing can be saved there.".into(),
        AlreadyExists => "A file with that name already exists.".into(),
        TimedOut => "The disk took too long to answer. Is it a network drive that went offline?".into(),
        _ => match e.raw_os_error() {
            // ENOSPC, EDQUOT, ENAMETOOLONG
            Some(28) | Some(122) | Some(69) => "The disk is full (or your storage quota is used up). Free up some space and resume.".into(),
            Some(36) | Some(63) => "That file name is too long for this disk. Use \"Save as\" to give it a shorter one.".into(),
            _ => format!("The disk reported a problem: {e}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_codes_say_what_to_do() {
        assert!(http_message(403).contains("expired") && http_message(403).contains("cookies"));
        assert!(http_message(401).contains("login"));
        assert!(http_message(404).contains("isn't there"));
        assert!(http_message(503).contains("keep trying"));
        assert!(http_message(418).contains("418"));
        // Every code produces a sentence, never an empty string.
        for c in 100..600 {
            assert!(http_message(c).len() > 10, "{c}");
        }
        assert!(Error::Http(404).to_string().contains("404"));
    }

    #[test]
    fn disk_problems_are_in_plain_words() {
        let io = |k| io_message(&std::io::Error::from(k));
        assert!(io(std::io::ErrorKind::PermissionDenied).contains("allowed"));
        assert!(io(std::io::ErrorKind::StorageFull).contains("full"));
        assert!(io(std::io::ErrorKind::NotFound).contains("destination folder"));
        let e = Error::DiskFull { need: 5 << 30, have: 1 << 30 };
        let m = e.to_string();
        assert!(m.contains("5.0 GB") && m.contains("1.0 GB"), "{m}");
    }

    #[test]
    fn checksum_message_explains_the_outcome() {
        let m = Error::Checksum { expected: "aa".into(), actual: "bb".into() }.to_string();
        assert!(m.contains("deleted") && m.contains("retry"));
    }

    #[tokio::test]
    async fn real_network_failures_get_real_explanations() {
        let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(4)).build().unwrap();
        // Nothing listening: connection refused.
        let e = client.get("http://127.0.0.1:9/x").send().await.unwrap_err();
        let m = net_message(&e);
        assert!(m.contains("refused the connection"), "{m}");
        // A name that can't exist: DNS.
        let e = client.get("http://definitely-not-a-real-host.invalid/x").send().await.unwrap_err();
        let m = net_message(&e);
        assert!(m.contains("Couldn't find") && m.contains("typos"), "{m}");
        // A plain-HTTP server answering a TLS hello: a secure connection can't be made.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                if let Ok((mut s, _)) = l.accept().await {
                    use tokio::io::AsyncWriteExt;
                    let _ = s.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
                }
            }
        });
        let e = client.get(format!("https://127.0.0.1:{port}/x")).send().await.unwrap_err();
        let m = net_message(&e);
        assert!(m.contains("Can't safely connect") || m.contains("cut off") || m.contains("Couldn't connect"), "{m}");
        // A server that accepts and never answers: timeout.
        let l2 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let p2 = l2.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            loop {
                if let Ok((s, _)) = l2.accept().await {
                    held.push(s);
                }
            }
        });
        let quick = reqwest::Client::builder().timeout(std::time::Duration::from_millis(400)).build().unwrap();
        let e = quick.get(format!("http://127.0.0.1:{p2}/x")).send().await.unwrap_err();
        assert!(net_message(&e).contains("took too long"), "{}", net_message(&e));
    }
}
