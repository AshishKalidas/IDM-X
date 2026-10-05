//! Hashing finished files and finding the checksum they should have.

use crate::error::{Error, Result};
use crate::types::ChecksumResult;
use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Algo {
    Sha256,
    Sha1,
    Md5,
    Blake3,
}

impl Algo {
    pub fn name(self) -> &'static str {
        match self {
            Algo::Sha256 => "sha256",
            Algo::Sha1 => "sha1",
            Algo::Md5 => "md5",
            Algo::Blake3 => "blake3",
        }
    }
    pub fn parse(s: &str) -> Option<Algo> {
        match s.to_ascii_lowercase().replace('-', "").as_str() {
            "sha256" => Some(Algo::Sha256),
            "sha1" => Some(Algo::Sha1),
            "md5" => Some(Algo::Md5),
            "blake3" | "b3" => Some(Algo::Blake3),
            _ => None,
        }
    }
    fn from_hex_len(n: usize) -> Option<Algo> {
        match n {
            32 => Some(Algo::Md5),
            40 => Some(Algo::Sha1),
            64 => Some(Algo::Sha256),
            _ => None,
        }
    }
}

/// What the user asked to verify against.
#[derive(Clone, Debug, PartialEq)]
pub enum Expected {
    Hash(Algo, String),
    /// Look for one the server publishes.
    Auto,
}

pub fn parse_expected(s: &str) -> Option<Expected> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if s.eq_ignore_ascii_case("auto") {
        return Some(Expected::Auto);
    }
    if let Some((a, h)) = s.split_once(':') {
        return Some(Expected::Hash(Algo::parse(a)?, h.trim().to_ascii_lowercase()));
    }
    let h = s.to_ascii_lowercase();
    if h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(Expected::Hash(Algo::from_hex_len(h.len())?, h));
    }
    None
}

/// Hash a file on the calling (blocking) thread, reporting progress as bytes hashed.
pub fn hash_file(path: &Path, algo: Algo, mut progress: impl FnMut(u64)) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    macro_rules! run {
        ($h:expr) => {{
            let mut h = $h;
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
                done += n as u64;
                progress(done);
            }
            h
        }};
    }
    Ok(match algo {
        Algo::Sha256 => hex::encode(run!(Sha256::new()).finalize()),
        Algo::Sha1 => hex::encode(run!(Sha1::new()).finalize()),
        Algo::Md5 => hex::encode(run!(Md5::new()).finalize()),
        Algo::Blake3 => run!(blake3::Hasher::new()).finalize().to_hex().to_string(),
    })
}

/// Read the hash for `file_name` out of a `.sha256`-style file: either a bare hash or
/// `<hash>  <name>` lines as written by `sha256sum`.
pub fn parse_sidecar(text: &str, file_name: &str) -> Option<String> {
    let mut first: Option<String> = None;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let mut parts = line.split_whitespace();
        let h = parts.next()?.trim_start_matches('\\').to_ascii_lowercase();
        if !h.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        match parts.next().map(|n| n.trim_start_matches('*')) {
            Some(n) if n == file_name => return Some(h),
            None => return Some(h),
            Some(_) => {
                first.get_or_insert(h);
            }
        }
    }
    // A single-entry file for a differently named mirror is still the best guess.
    first
}

pub fn compare(algo: Algo, actual: &str, expected: Option<(String, String)>) -> ChecksumResult {
    let (expected_hex, source) = match expected {
        Some((src, h)) => (Some(h), Some(src)),
        None => (None, None),
    };
    ChecksumResult {
        algo: algo.name().to_string(),
        actual: Some(actual.to_string()),
        verified: expected_hex.as_ref().map(|e| e.eq_ignore_ascii_case(actual)),
        expected: expected_hex,
        source,
    }
}

pub fn mismatch_error(r: &ChecksumResult) -> Error {
    Error::Checksum { expected: r.expected.clone().unwrap_or_default(), actual: r.actual.clone().unwrap_or_default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_expected() {
        assert_eq!(parse_expected("auto"), Some(Expected::Auto));
        assert_eq!(
            parse_expected("SHA256:ABCD"),
            Some(Expected::Hash(Algo::Sha256, "abcd".into()))
        );
        let h = "d41d8cd98f00b204e9800998ecf8427e";
        assert_eq!(parse_expected(h), Some(Expected::Hash(Algo::Md5, h.into())));
        assert_eq!(parse_expected("nonsense!"), None);
    }

    #[test]
    fn hashes_known_vectors() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(
            hash_file(&p, Algo::Sha256, |_| {}).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(hash_file(&p, Algo::Md5, |_| {}).unwrap(), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(hash_file(&p, Algo::Sha1, |_| {}).unwrap(), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn sidecar_formats() {
        let h = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(parse_sidecar(h, "x").as_deref(), Some(h));
        let multi = format!("{} other.bin\n{} *x.iso\n", "0".repeat(64), h);
        assert_eq!(parse_sidecar(&multi, "x.iso").as_deref(), Some(h));
    }
}
