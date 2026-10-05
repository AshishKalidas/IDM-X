//! Browser integration: extract the bundled extension, register the native-messaging host so the
//! extension can pair itself without anyone copying a token, and find installed browsers.

use anyhow::{Context, Result};
use rust_embed::RustEmbed;
use serde::Serialize;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const HOST_NAME: &str = "app.idmx.host";
/// Derived from the public key in `extension/manifest.json`, so it is the same on every machine.
pub const EXTENSION_ID: &str = "dobhgbonlcgnnfacapcpnhlkebclnjol";

#[derive(RustEmbed)]
#[folder = "../../extension"]
#[exclude = ".key-public.txt"]
#[exclude = "*.test.js"]
struct ExtensionFiles;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Browser {
    pub id: &'static str,
    pub name: &'static str,
    pub installed: bool,
    pub host_registered: bool,
}

struct Known {
    id: &'static str,
    name: &'static str,
    /// Per-OS user-data directory, relative to the home directory.
    mac: &'static str,
    linux: &'static str,
    mac_app: &'static str,
}

const KNOWN: &[Known] = &[
    Known { id: "chrome", name: "Google Chrome", mac: "Library/Application Support/Google/Chrome", linux: ".config/google-chrome", mac_app: "Google Chrome" },
    Known { id: "chromium", name: "Chromium", mac: "Library/Application Support/Chromium", linux: ".config/chromium", mac_app: "Chromium" },
    Known { id: "edge", name: "Microsoft Edge", mac: "Library/Application Support/Microsoft Edge", linux: ".config/microsoft-edge", mac_app: "Microsoft Edge" },
    Known { id: "brave", name: "Brave", mac: "Library/Application Support/BraveSoftware/Brave-Browser", linux: ".config/BraveSoftware/Brave-Browser", mac_app: "Brave Browser" },
    Known { id: "vivaldi", name: "Vivaldi", mac: "Library/Application Support/Vivaldi", linux: ".config/vivaldi", mac_app: "Vivaldi" },
    Known { id: "arc", name: "Arc", mac: "Library/Application Support/Arc/User Data", linux: "", mac_app: "Arc" },
];

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

fn user_data(k: &Known, home: &Path) -> Option<PathBuf> {
    let rel = if cfg!(target_os = "macos") { k.mac } else if cfg!(target_os = "linux") { k.linux } else { "" };
    (!rel.is_empty()).then(|| home.join(rel))
}

fn manifest_path(user_data: &Path) -> PathBuf {
    user_data.join("NativeMessagingHosts").join(format!("{HOST_NAME}.json"))
}

pub fn list_in(home: &Path) -> Vec<Browser> {
    KNOWN
        .iter()
        .map(|k| {
            let dir = user_data(k, home);
            Browser {
                id: k.id,
                name: k.name,
                installed: dir.as_ref().is_some_and(|d| d.exists()),
                host_registered: dir.as_ref().is_some_and(|d| manifest_path(d).exists()),
            }
        })
        .collect()
}

pub fn list() -> Vec<Browser> {
    home().map(|h| list_in(&h)).unwrap_or_default()
}

/// Write the extension's files to `<data_dir>/extension` (replacing older ones) and return that folder.
pub fn extract_extension(data_dir: &Path) -> Result<PathBuf> {
    let dir = data_dir.join("extension");
    let _ = std::fs::remove_dir_all(&dir);
    for name in ExtensionFiles::iter() {
        let file = ExtensionFiles::get(&name).context("missing embedded file")?;
        let dest = dir.join(name.as_ref());
        if let Some(p) = dest.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(dest, file.data)?;
    }
    Ok(dir)
}

/// Create the small launcher Chrome starts (it passes only the caller's origin), which runs
/// `<exe> --native-host`.
fn write_launcher(data_dir: &Path, exe: &Path) -> Result<PathBuf> {
    if cfg!(windows) {
        anyhow::bail!("automatic pairing isn't set up on Windows yet — paste the token in the extension instead");
    }
    let path = data_dir.join("idmx-native-host");
    let script = format!("#!/bin/sh\nexec \"{}\" --native-host \"$@\"\n", exe.display().to_string().replace('"', "\\\""));
    std::fs::write(&path, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(path)
}

/// Register the host with every installed browser; returns the ids that were registered.
pub fn register_host(home: &Path, data_dir: &Path, exe: &Path) -> Result<Vec<&'static str>> {
    let launcher = write_launcher(data_dir, exe)?;
    let manifest = serde_json::json!({
        "name": HOST_NAME,
        "description": "Lets the IDMX browser extension find the running app",
        "path": launcher,
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{EXTENSION_ID}/")],
    });
    let mut done = vec![];
    for k in KNOWN {
        let Some(dir) = user_data(k, home).filter(|d| d.exists()) else { continue };
        let file = manifest_path(&dir);
        std::fs::create_dir_all(file.parent().unwrap())?;
        std::fs::write(&file, serde_json::to_vec_pretty(&manifest)?)?;
        done.push(k.id);
    }
    Ok(done)
}

pub fn open_extensions_page(id: &str) -> Result<()> {
    let k = KNOWN.iter().find(|k| k.id == id).context("unknown browser")?;
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("open").args(["-a", k.mac_app, "chrome://extensions"]).status()?;
    #[cfg(not(target_os = "macos"))]
    let status = std::process::Command::new(match k.id { "chrome" => "google-chrome", "edge" => "microsoft-edge", "brave" => "brave-browser", other => other })
        .arg("chrome://extensions")
        .status()?;
    anyhow::ensure!(status.success(), "couldn't open {}", k.name);
    Ok(())
}

// ---- the native-messaging host itself ----

fn read_message(r: &mut impl Read) -> Option<serde_json::Value> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).ok()?;
    let len = u32::from_le_bytes(len) as usize;
    if len > 1024 * 1024 {
        return None;
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).ok()?;
    serde_json::from_slice(&buf).ok()
}

fn write_message(w: &mut impl Write, v: &serde_json::Value) -> std::io::Result<()> {
    let b = serde_json::to_vec(v)?;
    w.write_all(&(b.len() as u32).to_le_bytes())?;
    w.write_all(&b)?;
    w.flush()
}

/// Answer the extension's messages: `{type:"pair"}` → where the running app listens and its token.
/// Runs until the browser closes the pipe.
pub fn host_loop(data_dir: &Path, mut input: impl Read, mut output: impl Write) {
    while let Some(msg) = read_message(&mut input) {
        let reply = match msg.get("type").and_then(|t| t.as_str()) {
            Some("pair") => match crate::runtime_info(data_dir) {
                Some(i) => serde_json::json!({ "ok": true, "port": i.port, "token": i.token }),
                None => serde_json::json!({ "ok": false, "error": "IDMX isn't running" }),
            },
            _ => serde_json::json!({ "ok": false, "error": "unknown request" }),
        };
        if write_message(&mut output, &reply).is_err() {
            break;
        }
    }
}

pub fn run_native_host(data_dir: &Path) {
    host_loop(data_dir, std::io::stdin().lock(), std::io::stdout().lock());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_is_embedded_and_extracts() {
        let t = tempfile::tempdir().unwrap();
        let dir = extract_extension(t.path()).unwrap();
        assert!(dir.join("manifest.json").exists());
        assert!(!dir.join(".key-public.txt").exists());
    }

    #[test]
    fn registers_only_installed_browsers() {
        let t = tempfile::tempdir().unwrap();
        let k = &KNOWN[0];
        if let Some(d) = user_data(k, t.path()) {
            std::fs::create_dir_all(&d).unwrap();
            let data = t.path().join("data");
            std::fs::create_dir_all(&data).unwrap();
            let done = register_host(t.path(), &data, Path::new("/usr/bin/idmx")).unwrap();
            assert_eq!(done, vec!["chrome"]);
            let m: serde_json::Value = serde_json::from_slice(&std::fs::read(manifest_path(&d)).unwrap()).unwrap();
            assert_eq!(m["allowed_origins"][0], format!("chrome-extension://{EXTENSION_ID}/"));
            assert!(list_in(t.path())[0].host_registered);
        }
    }

    #[test]
    fn host_answers_pair() {
        let t = tempfile::tempdir().unwrap();
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::fs::write(t.path().join("runtime.json"), format!(r#"{{"port":{port},"token":"abc","pid":1}}"#)).unwrap();
        let mut inp = vec![];
        write_message(&mut inp, &serde_json::json!({"type":"pair"})).unwrap();
        let mut out = vec![];
        host_loop(t.path(), &inp[..], &mut out);
        let reply = read_message(&mut &out[..]).unwrap();
        assert_eq!(reply["token"], "abc");
        assert_eq!(reply["port"], port);
    }
}
