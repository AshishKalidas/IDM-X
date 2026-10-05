//! The few things that need to talk to the operating system.

use std::path::Path;
use std::process::Command;

/// Show a file in Finder / Explorer / the file manager.
pub fn reveal(path: &Path) {
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let _ = Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn();
}

/// Open a web page in the default browser. Only `http(s)` links are accepted.
pub fn open_url(url: &str) -> bool {
    if !(url.starts_with("https://") || url.starts_with("http://")) || url.chars().any(|c| c.is_control() || c == ' ') {
        return false;
    }
    #[cfg(target_os = "macos")]
    let r = Command::new("open").arg(url).spawn();
    // `start` would treat `&` in a URL as a command separator; the shell-extension handler doesn't.
    #[cfg(target_os = "windows")]
    let r = Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = Command::new("xdg-open").arg(url).spawn();
    r.is_ok()
}

pub fn open_path(path: &Path) {
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let _ = Command::new("cmd").args(["/C", "start", ""]).arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = Command::new("xdg-open").arg(path).spawn();
}

/// Open the operating system's network settings.
pub fn open_network_settings() {
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg("x-apple.systempreferences:com.apple.Network-Settings.extension").spawn().or_else(|_| Command::new("open").arg("x-apple.systempreferences:com.apple.preference.network").spawn());
    #[cfg(target_os = "windows")]
    let _ = Command::new("cmd").args(["/C", "start", "", "ms-settings:network-status"]).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // Whichever the desktop has; the first that exists wins.
        for (cmd, args) in [("gnome-control-center", vec!["network"]), ("nm-connection-editor", vec![]), ("systemsettings", vec!["kcm_networkmanagement"])] {
            if Command::new(cmd).args(args).spawn().is_ok() {
                break;
            }
        }
    }
}

pub fn sleep_system() {
    #[cfg(target_os = "macos")]
    let _ = Command::new("pmset").arg("sleepnow").spawn();
    #[cfg(target_os = "windows")]
    let _ = Command::new("rundll32.exe").args(["powrprof.dll,SetSuspendState", "0,1,0"]).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = Command::new("systemctl").arg("suspend").spawn();
}

/// Keep the machine awake while downloads run. Returns a guard; dropping it lets it sleep again.
pub struct AwakeGuard {
    #[allow(dead_code)]
    child: Option<std::process::Child>,
    /// Windows: the thread that holds the execution state, and how to tell it to let go.
    #[cfg(windows)]
    release: Option<std::sync::mpsc::Sender<()>>,
}

pub fn keep_awake() -> AwakeGuard {
    #[cfg(target_os = "macos")]
    {
        // `caffeinate -i` prevents idle sleep for as long as the child lives.
        return AwakeGuard { child: Command::new("caffeinate").arg("-i").spawn().ok() };
    }
    #[cfg(target_os = "linux")]
    {
        return AwakeGuard {
            child: Command::new("systemd-inhibit").args(["--what=idle:sleep", "--who=IDMX", "--why=Downloading", "sleep", "infinity"]).spawn().ok(),
        };
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED};
        // The execution state belongs to the thread that sets it, so a dedicated thread holds it
        // until told to let go.
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
            let _ = rx.recv();
            unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
        });
        return AwakeGuard { child: None, release: Some(tx) };
    }
    #[allow(unreachable_code)]
    AwakeGuard {
        child: None,
        #[cfg(windows)]
        release: None,
    }
}

impl Drop for AwakeGuard {
    fn drop(&mut self) {
        if let Some(c) = self.child.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
        #[cfg(windows)]
        if let Some(tx) = self.release.take() {
            let _ = tx.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_web_links_are_opened() {
        assert!(!super::open_url("file:///etc/passwd"));
        assert!(!super::open_url("javascript:alert(1)"));
        assert!(!super::open_url("https://x.com/a b"));
        assert!(!super::open_url("https://x.com/\nwhoami"));
    }
}
