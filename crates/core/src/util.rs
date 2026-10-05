use std::path::{Path, PathBuf};

/// Make a server-supplied file name safe on every OS.
pub fn sanitize_filename(raw: &str) -> String {
    let name = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            c if (c as u32) < 32 => '_',
            c => c,
        })
        .collect();
    out = out.trim().trim_end_matches(['.', ' ']).to_string();
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
        "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&stem.as_str()) {
        out.insert(0, '_');
    }
    if out.is_empty() || out == "." || out == ".." {
        return "download".into();
    }
    // Keep names under common 255-byte limits while preserving the extension.
    if out.len() > 200 {
        let (stem, ext) = split_ext(&out);
        let keep = 200usize.saturating_sub(ext.len());
        let mut cut = keep.min(stem.len());
        while !stem.is_char_boundary(cut) {
            cut -= 1;
        }
        out = format!("{}{}", &stem[..cut], ext);
    }
    out
}

/// A video's or song's title as a file name. Unlike a name from a server, a title is free text:
/// a slash in it ("AC/DC", "Part 1/2") is just a character, not a folder.
pub fn sanitize_title(title: &str) -> String {
    sanitize_filename(&title.replace(['/', '\\'], "-"))
}

/// `("archive.tar", ".gz")` for `archive.tar.gz`-style names is not attempted: just last dot.
pub fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 && name.len() - i <= 12 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

pub fn extension(name: &str) -> String {
    split_ext(name).1.trim_start_matches('.').to_ascii_lowercase()
}

/// `report.pdf` → `report (1).pdf` → `report (2).pdf`, skipping names that exist or are `taken`.
pub fn unique_name(dir: &Path, name: &str, taken: impl Fn(&str) -> bool) -> String {
    let free = |n: &str| !dir.join(n).exists() && !dir.join(format!("{n}.idmx")).exists() && !taken(n);
    if free(name) {
        return name.to_string();
    }
    let (stem, ext) = split_ext(name);
    (1..10_000)
        .map(|i| format!("{stem} ({i}){ext}"))
        .find(|n| free(n))
        .unwrap_or_else(|| format!("{stem}-{}{ext}", uuid::Uuid::new_v4().simple()))
}

pub fn staging_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.idmx"))
}

pub fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(unix)]
pub fn write_all_at(file: &std::fs::File, mut buf: &[u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    while !buf.is_empty() {
        let n = file.write_at(buf, offset)?;
        if n == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        buf = &buf[n..];
        offset += n as u64;
    }
    Ok(())
}

#[cfg(windows)]
pub fn write_all_at(file: &std::fs::File, mut buf: &[u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = file.seek_write(buf, offset)?;
        if n == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        buf = &buf[n..];
        offset += n as u64;
    }
    Ok(())
}

pub fn human_bytes(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{n} B") } else { format!("{v:.1} {}", U[i]) }
}

/// Parse `5M`, `800k`, `1.5G`, `250000` into bytes.
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_lowercase();
    let s = s.trim_end_matches('b').trim_end_matches('i');
    let (num, mult) = match s.chars().last()? {
        'k' => (&s[..s.len() - 1], 1u64 << 10),
        'm' => (&s[..s.len() - 1], 1 << 20),
        'g' => (&s[..s.len() - 1], 1 << 30),
        't' => (&s[..s.len() - 1], 1 << 40),
        _ => (s, 1),
    };
    num.trim().parse::<f64>().ok().map(|v| (v * mult as f64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_keep_their_slashes_as_dashes() {
        assert_eq!(sanitize_title("AC/DC - Thunderstruck"), "AC-DC - Thunderstruck");
        assert_eq!(sanitize_title("Part 1/2: the end?"), "Part 1-2_ the end_");
        assert_eq!(sanitize_title("///"), "---");
        assert_eq!(sanitize_title(""), "download");
    }

    #[test]
    fn sanitizes() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(sanitize_filename("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_filename("   "), "download");
        assert_eq!(sanitize_filename("name. "), "name");
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_size("5M"), Some(5 << 20));
        assert_eq!(parse_size("1.5g"), Some((1.5 * (1u64 << 30) as f64) as u64));
        assert_eq!(parse_size("800KB"), Some(800 << 10));
        assert_eq!(parse_size("1234"), Some(1234));
        assert_eq!(parse_size("x"), None);
    }

    #[test]
    fn unique_names_skip_taken() {
        let dir = std::env::temp_dir().join("idmx-test-unique");
        let _ = std::fs::create_dir_all(&dir);
        let n = unique_name(&dir, "x.bin", |n| n == "x.bin");
        assert_eq!(n, "x (1).bin");
    }
}
