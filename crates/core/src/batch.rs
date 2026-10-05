//! Turning pasted text into a list of URLs: line lists, `curl`-style globbing, Metalink.

use regex::Regex;

const MAX_EXPANSION: usize = 20_000;

/// Split pasted text into URLs, drop comments and blanks, and expand patterns:
///
/// * `file[01-20].zip`  numeric ranges (zero padding is kept)
/// * `img[a-e].png`     letter ranges
/// * `v{1.0,1.1,2.0}`   explicit alternatives
pub fn expand(input: &str) -> Vec<String> {
    expand_items(input).into_iter().map(|i| i.url).collect()
}

/// One thing to download, plus anything we learned about it from the list it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub url: String,
    /// `sha256:<hex>` from a Metalink.
    pub checksum: Option<String>,
    pub name: Option<String>,
}

pub fn expand_items(input: &str) -> Vec<Item> {
    let trimmed = input.trim_start();
    if trimmed.starts_with("<?xml") && trimmed.contains("<metalink") {
        return parse_metalink(input)
            .into_iter()
            .map(|m| Item { url: m.url, checksum: m.hash.map(|(a, h)| format!("{a}:{h}")), name: Some(m.name) })
            .collect();
    }
    let mut out: Vec<Item> = Vec::new();
    for token in input.lines().filter(|l| !l.trim_start().starts_with('#')).flat_map(|l| {
        // A `.torrent` file on this computer is one item even if its path has spaces or brackets.
        let t = l.trim();
        if matches!(crate::torrent::Source::parse(t), Some(crate::torrent::Source::File(_))) {
            vec![t]
        } else {
            l.split_whitespace().collect()
        }
    }) {
        if matches!(crate::torrent::Source::parse(token), Some(crate::torrent::Source::File(_))) {
            out.push(Item { url: token.to_string(), checksum: None, name: None });
            continue;
        }
        for u in expand_pattern(token) {
            out.push(Item { url: u, checksum: None, name: None });
            if out.len() >= MAX_EXPANSION {
                return out;
            }
        }
    }
    out
}

pub fn expand_pattern(s: &str) -> Vec<String> {
    // Find the first bracket group; recurse on the remainder for a cartesian product.
    let b = s.find('[');
    let c = s.find('{');
    let (open, close, curly) = match (b, c) {
        (Some(b), Some(c)) if c < b => (c, '}', true),
        (Some(b), _) => (b, ']', false),
        (None, Some(c)) => (c, '}', true),
        (None, None) => return vec![s.to_string()],
    };
    let Some(end_rel) = s[open..].find(close) else { return vec![s.to_string()] };
    let end = open + end_rel;
    let body = &s[open + 1..end];
    let (head, tail) = (&s[..open], &s[end + 1..]);

    let choices: Vec<String> = if curly {
        body.split(',').map(str::to_string).collect()
    } else {
        match range_choices(body) {
            Some(c) => c,
            // Not a range (IPv6 literal, etc.): leave the brackets alone.
            None => {
                return expand_pattern(tail)
                    .into_iter()
                    .map(|t| format!("{head}[{body}]{t}"))
                    .collect()
            }
        }
    };
    let tails = expand_pattern(tail);
    let mut out = Vec::with_capacity(choices.len() * tails.len());
    'outer: for ch in &choices {
        for t in &tails {
            out.push(format!("{head}{ch}{t}"));
            if out.len() >= MAX_EXPANSION {
                break 'outer;
            }
        }
    }
    out
}

fn range_choices(body: &str) -> Option<Vec<String>> {
    let (a, b) = body.split_once('-')?;
    if let (Ok(x), Ok(y)) = (a.parse::<u64>(), b.parse::<u64>()) {
        if y < x || y - x > 100_000 {
            return None;
        }
        let width = if a.starts_with('0') && a.len() > 1 { a.len() } else { 0 };
        return Some((x..=y).map(|n| format!("{n:0width$}")).collect());
    }
    let (mut ca, mut cb) = (a.chars(), b.chars());
    let (x, y) = (ca.next()?, cb.next()?);
    if ca.next().is_some() || cb.next().is_some() || !x.is_ascii_alphabetic() || y < x {
        return None;
    }
    Some((x..=y).map(String::from).collect())
}

#[derive(Debug, Clone, PartialEq)]
pub struct MetalinkFile {
    pub name: String,
    pub url: String,
    /// `(algo, hex)` — the strongest hash the file lists.
    pub hash: Option<(String, String)>,
}

/// Just enough Metalink v4 (`.meta4`) parsing to queue files with their published hashes.
pub fn parse_metalink(xml: &str) -> Vec<MetalinkFile> {
    let file_re = Regex::new(r#"(?s)<file\s+name="([^"]+)"\s*>(.*?)</file>"#).unwrap();
    let url_re = Regex::new(r#"(?s)<url[^>]*>\s*([^<\s]+)\s*</url>"#).unwrap();
    let hash_re = Regex::new(r#"(?s)<hash\s+type="([^"]+)"\s*>\s*([0-9a-fA-F]+)\s*</hash>"#).unwrap();
    file_re
        .captures_iter(xml)
        .filter_map(|c| {
            let url = url_re.captures(&c[2])?[1].to_string();
            let hash = hash_re
                .captures_iter(&c[2])
                .map(|h| (h[1].to_ascii_lowercase().replace('-', ""), h[2].to_ascii_lowercase()))
                .find(|(a, _)| a == "sha256")
                .or_else(|| {
                    hash_re
                        .captures_iter(&c[2])
                        .map(|h| (h[1].to_ascii_lowercase().replace('-', ""), h[2].to_ascii_lowercase()))
                        .next()
                });
            Some(MetalinkFile { name: c[1].to_string(), url, hash })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_torrent_file_path_with_spaces_is_one_item() {
        let d = std::env::temp_dir().join("idmx test dir [1]");
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("my file.torrent");
        std::fs::write(&f, b"d").unwrap();
        let items = super::expand_items(&f.display().to_string());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].url, f.display().to_string());
        let _ = std::fs::remove_dir_all(&d);
    }
    use super::*;

    #[test]
    fn numeric_range_keeps_padding() {
        let v = expand_pattern("https://x/f[08-11].zip");
        assert_eq!(v, ["https://x/f08.zip", "https://x/f09.zip", "https://x/f10.zip", "https://x/f11.zip"]);
    }

    #[test]
    fn cartesian_and_alternatives() {
        let v = expand_pattern("a[1-2]-{x,y}");
        assert_eq!(v, ["a1-x", "a1-y", "a2-x", "a2-y"]);
    }

    #[test]
    fn letter_range() {
        assert_eq!(expand_pattern("p[a-c]"), ["pa", "pb", "pc"]);
    }

    #[test]
    fn ipv6_literal_untouched() {
        assert_eq!(expand_pattern("http://[::1]:8080/x"), ["http://[::1]:8080/x"]);
    }

    #[test]
    fn lists_and_comments() {
        let v = expand("# hi\nhttp://a/1\n\nhttp://a/2 http://a/3");
        assert_eq!(v.len(), 3);
    }

    #[test]
    fn metalink() {
        let xml = r#"<?xml version="1.0"?><metalink xmlns="urn:ietf:params:xml:ns:metalink"><file name="a.iso"><hash type="sha-256">ABCD</hash><url>http://h/a.iso</url></file></metalink>"#;
        let f = parse_metalink(xml);
        assert_eq!(f[0].name, "a.iso");
        assert_eq!(f[0].hash, Some(("sha256".into(), "abcd".into())));
        assert_eq!(expand(xml), ["http://h/a.iso"]);
    }
}
