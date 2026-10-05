//! Finding the machine's networks and working out what each one *is*.

use crate::types::{IfKind, NetInterface};
use netdev::interface::types::InterfaceType;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Everything the OS will let us bind to, blocking. Call from `spawn_blocking`.
pub fn discover() -> Vec<NetInterface> {
    let hw = hardware_ports();
    let mut out: Vec<NetInterface> = netdev::get_interfaces()
        .into_iter()
        .map(|i| {
            let ipv4: Vec<Ipv4Addr> = i.ipv4.iter().map(|n| n.addr()).collect();
            let ipv6: Vec<Ipv6Addr> = i.ipv6.iter().map(|n| n.addr()).collect();
            // macOS keys hardware ports by device name; Windows by the adapter's friendly name.
            let hw_entry = hw.get(&i.name).or_else(|| i.friendly_name.as_ref().and_then(|f| hw.get(f)));
            let hw_label = hw_entry.map(|(l, _)| l.as_str());
            let text = [
                hw_label.unwrap_or(""),
                hw_entry.map(|(_, x)| x.as_str()).unwrap_or(""),
                i.friendly_name.as_deref().unwrap_or(""),
                i.description.as_deref().unwrap_or(""),
            ]
            .join(" ");
            let kind = classify(&i.name, &text, i.if_type, i.is_loopback());
            let label = hw_label
                .map(str::to_string)
                .or_else(|| i.friendly_name.clone())
                .unwrap_or_else(|| default_label(&i.name, kind));
            let usable = i.is_up()
                && !i.is_loopback()
                && (ipv4.iter().any(|a| usable_v4(*a)) || ipv6.iter().any(|a| usable_v6(*a)));
            let mut subnets: Vec<String> = i
                .ipv4
                .iter()
                .filter(|n| usable_v4(n.addr()))
                .map(|n| format!("{}/{}", n.network(), n.prefix_len()))
                .collect();
            subnets.sort();
            subnets.dedup();
            NetInterface {
                id: i.name.clone(),
                label,
                kind,
                subnets,
                ipv4: ipv4.iter().filter(|a| usable_v4(**a)).map(|a| a.to_string()).collect(),
                ipv6: ipv6.iter().filter(|a| usable_v6(**a)).map(|a| a.to_string()).collect(),
                gateway: i
                    .gateway
                    .as_ref()
                    .and_then(|g| g.ipv4.first().map(|a| a.to_string()).or_else(|| g.ipv6.first().map(|a| a.to_string()))),
                mac: i.mac_addr.map(|m| m.to_string()).filter(|m| m != "00:00:00:00:00:00"),
                up: i.is_up(),
                link_mbps: i.transmit_speed.map(|b| b / 1_000_000).filter(|m| *m > 0),
                usable,
            }
        })
        .collect();
    // Stable order: real connections first, plumbing last.
    out.sort_by_key(|n| (!n.usable, rank(n.kind), n.id.clone()));
    out
}

fn rank(k: IfKind) -> u8 {
    match k {
        IfKind::Ethernet => 0,
        IfKind::Wifi => 1,
        IfKind::Tether => 2,
        IfKind::Cellular => 3,
        IfKind::Unknown => 4,
        IfKind::Vpn => 5,
        IfKind::Bridge => 6,
        IfKind::Virtual => 7,
        IfKind::Loopback => 8,
    }
}

pub fn usable_v4(a: Ipv4Addr) -> bool {
    !(a.is_loopback() || a.is_unspecified() || a.is_link_local() || a.is_multicast() || a.is_broadcast())
}

pub fn usable_v6(a: Ipv6Addr) -> bool {
    // Only global unicast (2000::/3): link-local and ULA addresses don't reach the internet.
    (a.segments()[0] & 0xe000) == 0x2000
}

fn default_label(name: &str, kind: IfKind) -> String {
    match kind {
        IfKind::Wifi => "Wi-Fi".into(),
        IfKind::Ethernet => format!("Ethernet ({name})"),
        IfKind::Cellular => "Cellular".into(),
        IfKind::Tether => format!("USB tether ({name})"),
        IfKind::Vpn => format!("VPN ({name})"),
        IfKind::Loopback => "Loopback".into(),
        _ => name.to_string(),
    }
}

fn classify(name: &str, text: &str, ty: InterfaceType, loopback: bool) -> IfKind {
    let n = name.to_ascii_lowercase();
    let t = text.to_ascii_lowercase();
    // Whole-word match so "Realtek" is not mistaken for "LTE".
    let words: Vec<&str> = t.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |needles: &[&str]| {
        needles.iter().any(|x| {
            if x.contains([' ', '-', '.']) { t.contains(x) } else { words.iter().any(|w| w == x) }
        })
    };
    let starts = |ps: &[&str]| ps.iter().any(|p| n.starts_with(p));

    if loopback || ty == InterfaceType::Loopback || n == "lo" {
        return IfKind::Loopback;
    }
    if starts(&["utun", "tun", "tap", "wg", "ppp", "ipsec", "tailscale", "zt", "gif", "stf"])
        || has(&["vpn", "wireguard", "tap-windows", "tailscale", "zerotier", "wintun", "anyconnect", "globalprotect"])
    {
        return IfKind::Vpn;
    }
    if has(&["rndis", "remote ndis", "iphone", "ipad", "android", "tether", "usb"]) || starts(&["ipheth", "rndis", "usb"]) {
        return IfKind::Tether;
    }
    if has(&["thunderbolt bridge"]) || starts(&["bridge", "br-", "virbr"]) && !has(&["wi-fi"]) {
        return IfKind::Bridge;
    }
    if starts(&["awdl", "llw", "ap", "anpi", "nan", "p2p", "docker", "veth", "vmnet", "vboxnet", "lxc", "vnic", "utap"])
        || has(&["hyper-v", "vethernet", "vmware", "virtualbox", "wsl", "bluetooth", "virtual", "loopback pseudo"])
        || ty == InterfaceType::PeerToPeerWireless
    {
        return IfKind::Virtual;
    }
    if has(&["wi-fi", "wifi", "wireless", "wlan", "airport", "802.11"]) || ty == InterfaceType::Wireless80211 || starts(&["wlan", "wlp", "wlx"]) {
        return IfKind::Wifi;
    }
    if has(&["cellular", "mobile broadband", "wwan", "lte", "5g", "modem"])
        || matches!(ty, InterfaceType::Wwan | InterfaceType::Wwanpp | InterfaceType::Wwanpp2 | InterfaceType::Wman)
        || starts(&["wwan", "wwp", "rmnet", "ccmni"])
    {
        return IfKind::Cellular;
    }
    #[cfg(target_os = "linux")]
    {
        if std::path::Path::new(&format!("/sys/class/net/{name}/wireless")).exists() {
            return IfKind::Wifi;
        }
        if let Ok(u) = std::fs::read_to_string(format!("/sys/class/net/{name}/device/uevent")) {
            if u.contains("DRIVER=rndis_host") || u.contains("DRIVER=cdc_ether") || u.contains("DRIVER=ipheth") {
                return IfKind::Tether;
            }
        }
    }
    if has(&["ethernet", "lan"]) || starts(&["eth", "enp", "eno", "ens", "en"]) || matches!(ty, InterfaceType::Ethernet | InterfaceType::GigabitEthernet | InterfaceType::FastEthernetT | InterfaceType::FastEthernetFx) {
        return IfKind::Ethernet;
    }
    IfKind::Unknown
}

/// Friendly names for adapters: `device → (label, extra text for classification)`.
/// macOS: `en0` → "Wi-Fi". Windows: `Get-NetAdapter`, keyed by the adapter's name. Empty elsewhere.
fn hardware_ports() -> HashMap<String, (String, String)> {
    #[cfg(target_os = "macos")]
    {
        let Ok(out) = std::process::Command::new("networksetup").arg("-listallhardwareports").output() else {
            return HashMap::new();
        };
        return parse_hardware_ports(&String::from_utf8_lossy(&out.stdout));
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let script = "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; Get-NetAdapter -ErrorAction Stop | Select-Object Name,InterfaceDescription,NdisPhysicalMedium | ConvertTo-Json -Compress";
        let Ok(out) = std::process::Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", script]).creation_flags(CREATE_NO_WINDOW).output() else {
            return HashMap::new();
        };
        return parse_windows_adapters(&String::from_utf8_lossy(&out.stdout));
    }
    #[allow(unreachable_code)]
    HashMap::new()
}

pub fn parse_hardware_ports(text: &str) -> HashMap<String, (String, String)> {
    let mut map = HashMap::new();
    let mut port: Option<String> = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("Hardware Port: ") {
            port = Some(p.trim().to_string());
        } else if let Some(d) = line.strip_prefix("Device: ") {
            if let Some(p) = port.take() {
                map.insert(d.trim().to_string(), (p, String::new()));
            }
        }
    }
    map
}

/// `Get-NetAdapter | ConvertTo-Json` gives one object for one adapter and an array for several.
/// `NdisPhysicalMedium` is the numeric enum; it is what tells a Wi-Fi card from a cable.
pub fn parse_windows_adapters(json: &str) -> HashMap<String, (String, String)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json.trim().trim_start_matches('\u{feff}')) else {
        return HashMap::new();
    };
    let items: Vec<serde_json::Value> = match v {
        serde_json::Value::Array(a) => a,
        o @ serde_json::Value::Object(_) => vec![o],
        _ => vec![],
    };
    items
        .into_iter()
        .filter_map(|i| {
            let name = i.get("Name")?.as_str()?.to_string();
            let desc = i.get("InterfaceDescription").and_then(|d| d.as_str()).unwrap_or("");
            let medium = match i.get("NdisPhysicalMedium").and_then(|m| m.as_i64()) {
                Some(9) | Some(1) => "Wi-Fi 802.11",
                Some(8) => "Mobile broadband wwan",
                Some(10) => "Bluetooth",
                Some(14) => "Ethernet 802.3",
                _ => "",
            };
            Some((name.clone(), (name, format!("{desc} {medium}"))))
        })
        .collect()
}

/// Which local address to bind for a given server address family.
pub fn local_addr(iface: &NetInterface, want_v6: bool) -> Option<IpAddr> {
    let v4 = iface.ipv4.first().and_then(|a| a.parse().ok()).map(IpAddr::V4);
    let v6 = iface.ipv6.first().and_then(|a| a.parse().ok()).map(IpAddr::V6);
    if want_v6 {
        v6.or(v4)
    } else {
        v4.or(v6)
    }
}

/// A fingerprint for change detection: anything that should rebuild connections.
pub fn fingerprint(n: &NetInterface) -> String {
    format!("{}|{}|{}|{:?}|{}", n.id, n.up, n.usable, n.ipv4, n.ipv6.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies() {
        let k = |n: &str, t: &str, ty| classify(n, t, ty, false);
        assert_eq!(k("en0", "Wi-Fi", InterfaceType::Ethernet), IfKind::Wifi);
        assert_eq!(k("en5", "iPhone USB", InterfaceType::Ethernet), IfKind::Tether);
        assert_eq!(k("utun3", "", InterfaceType::Unknown), IfKind::Vpn);
        assert_eq!(k("bridge0", "Thunderbolt Bridge", InterfaceType::Bridge), IfKind::Bridge);
        assert_eq!(k("awdl0", "", InterfaceType::Unknown), IfKind::Virtual);
        assert_eq!(k("eth0", "", InterfaceType::Ethernet), IfKind::Ethernet);
        assert_eq!(k("Ethernet 2", "Realtek PCIe GbE Family Controller", InterfaceType::Ethernet), IfKind::Ethernet);
        assert_eq!(k("Ethernet 3", "Remote NDIS based Internet Sharing Device", InterfaceType::Ethernet), IfKind::Tether);
        assert_eq!(k("wlp2s0", "", InterfaceType::Unknown), IfKind::Wifi);
    }

    #[test]
    fn parses_macos_ports() {
        let t = "Hardware Port: Wi-Fi\nDevice: en0\nEthernet Address: aa\n\nHardware Port: iPhone USB\nDevice: en5\n";
        let m = parse_hardware_ports(t);
        assert_eq!(m["en0"].0, "Wi-Fi");
        assert_eq!(m["en5"].0, "iPhone USB");
    }

    #[test]
    fn parses_windows_adapters_one_or_many() {
        let many = r#"[{"Name":"Wi-Fi","InterfaceDescription":"Intel(R) Wi-Fi 6 AX201","NdisPhysicalMedium":9},{"Name":"Ethernet 3","InterfaceDescription":"Remote NDIS based Internet Sharing Device","NdisPhysicalMedium":14}]"#;
        let m = parse_windows_adapters(many);
        assert_eq!(m.len(), 2);
        // The medium and description feed the classifier: an RNDIS "Ethernet" is a phone.
        let (label, extra) = &m["Ethernet 3"];
        assert_eq!(label, "Ethernet 3");
        assert_eq!(classify("{guid}", &format!("{label} {extra}"), InterfaceType::Ethernet, false), IfKind::Tether);
        let (l, x) = &m["Wi-Fi"];
        assert_eq!(classify("{guid}", &format!("{l} {x}"), InterfaceType::Ethernet, false), IfKind::Wifi);
        // A single adapter arrives as a bare object, and a BOM must not break parsing.
        let one = "\u{feff}{\"Name\":\"Ethernet\",\"InterfaceDescription\":\"Realtek PCIe GbE\",\"NdisPhysicalMedium\":14}";
        assert_eq!(parse_windows_adapters(one).len(), 1);
        assert!(parse_windows_adapters("garbage").is_empty());
    }

    #[test]
    fn address_filters() {
        assert!(usable_v4("192.168.1.5".parse().unwrap()));
        assert!(!usable_v4("169.254.1.1".parse().unwrap()));
        assert!(!usable_v6("fe80::1".parse().unwrap()));
        assert!(usable_v6("2001:db8::1".parse().unwrap()));
    }
}
