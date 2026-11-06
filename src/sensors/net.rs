//! Netzwerkdurchsatz über GetIfTable2 – nur auf einem gewählten (physischen) Adapter,
//! damit VPN- und virtuelle Adapter nicht doppelt zählen.

use std::time::Instant;
#[cfg(windows)]
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};

#[derive(Debug, Clone, serde::Serialize)]
pub struct Adapter {
    pub guid: String,
    pub name: String,
    pub description: String,
    pub hardware: bool,
    pub up: bool,
    pub in_octets: u64,
    pub out_octets: u64,
    pub rx_link_bps: u64,
    pub tx_link_bps: u64,
}

#[cfg(windows)]
pub fn list_adapters() -> Vec<Adapter> {
    let mut out = Vec::new();
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    unsafe {
        if GetIfTable2(&mut table).is_err() || table.is_null() {
            return out;
        }
        let n = (*table).NumEntries as usize;
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), n);
        for r in rows {
            let flags = r.InterfaceAndOperStatusFlags._bitfield;
            let hardware_if = flags & 0x1 != 0;
            let filter_if = flags & 0x2 != 0;
            if filter_if {
                continue; // Filter-Schnittstellen (WFP, QoS …) duplizieren nur echte Adapter
            }
            let name = String::from_utf16_lossy(&r.Alias[..r.Alias.iter().position(|c| *c == 0).unwrap_or(r.Alias.len())]);
            let description =
                String::from_utf16_lossy(&r.Description[..r.Description.iter().position(|c| *c == 0).unwrap_or(r.Description.len())]);
            // 24 = Loopback, 131 = Tunnel
            let hardware = hardware_if && r.Type != 24 && r.Type != 131;
            out.push(Adapter {
                guid: format!("{:?}", r.InterfaceGuid),
                name,
                description,
                hardware,
                up: r.OperStatus.0 == 1,
                in_octets: r.InOctets,
                out_octets: r.OutOctets,
                rx_link_bps: r.ReceiveLinkSpeed,
                tx_link_bps: r.TransmitLinkSpeed,
            });
        }
        FreeMibTable(table as *const _);
    }
    out
}

#[cfg_attr(windows, allow(dead_code))]
/// Eine Zeile aus /proc/net/dev → (Name, empfangene Bytes, gesendete Bytes)
pub fn parse_net_dev_line(line: &str) -> Option<(String, u64, u64)> {
    let (name, rest) = line.split_once(':')?;
    let v: Vec<u64> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    (v.len() >= 9).then(|| (name.trim().to_string(), v[0], v[8]))
}

/// Linux: Adapter aus /proc/net/dev, Details aus /sys/class/net/<name>
#[cfg(target_os = "linux")]
pub fn list_adapters() -> Vec<Adapter> {
    use super::sysfs::{read, read_i64};
    let Ok(dev) = std::fs::read_to_string("/proc/net/dev") else { return Vec::new() };
    dev.lines()
        .skip(2)
        .filter_map(parse_net_dev_line)
        .map(|(name, rx, tx)| {
            let base = format!("/sys/class/net/{name}");
            // Physisch = hat ein Gerät (PCI/USB); lo, docker0, veth*, tun* nicht
            let hardware = std::path::Path::new(&format!("{base}/device")).exists();
            let wireless = std::path::Path::new(&format!("{base}/wireless")).exists();
            let speed = read_i64(format!("{base}/speed")).filter(|s| *s > 0).map(|s| s as u64 * 1_000_000).unwrap_or(0);
            Adapter {
                guid: name.clone(),
                description: if wireless { format!("WLAN ({name})") } else if hardware { format!("Ethernet ({name})") } else { name.clone() },
                name,
                hardware,
                up: read(format!("{base}/operstate")).map(|s| s == "up").unwrap_or(false),
                in_octets: rx,
                out_octets: tx,
                rx_link_bps: speed,
                tx_link_bps: speed,
            }
        })
        .collect()
}

/// Automatische Wahl: aktiver physischer Adapter mit dem meisten Verkehr.
pub fn auto_adapter(list: &[Adapter]) -> Option<&Adapter> {
    list.iter().filter(|a| a.hardware && a.up).max_by_key(|a| a.in_octets.saturating_add(a.out_octets))
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct NetSample {
    pub adapter_name: Option<String>,
    pub down_bps: Option<f64>,
    pub up_bps: Option<f64>,
    pub rx_link_bps: u64,
    pub tx_link_bps: u64,
}

pub struct Net {
    last: Option<(String, u64, u64, Instant)>,
}

impl Net {
    pub fn new() -> Net {
        Net { last: None }
    }

    /// `wanted`: GUID aus den Einstellungen, leer = automatisch.
    pub fn sample(&mut self, wanted: &str) -> NetSample {
        let list = list_adapters();
        let chosen = if wanted.is_empty() { auto_adapter(&list) } else { list.iter().find(|a| a.guid == wanted) };
        let Some(a) = chosen else {
            self.last = None;
            return NetSample::default();
        };
        let now = Instant::now();
        let mut s = NetSample {
            adapter_name: Some(a.name.clone()),
            rx_link_bps: a.rx_link_bps,
            tx_link_bps: a.tx_link_bps,
            ..Default::default()
        };
        if let Some((guid, rin, rout, t)) = &self.last {
            if *guid == a.guid {
                let dt = now.duration_since(*t).as_secs_f64();
                if dt > 0.05 {
                    s.down_bps = Some(a.in_octets.saturating_sub(*rin) as f64 / dt);
                    s.up_bps = Some(a.out_octets.saturating_sub(*rout) as f64 / dt);
                }
            }
        }
        self.last = Some((a.guid.clone(), a.in_octets, a.out_octets, now));
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_net_dev() {
        let l = "  eth0: 123456 100 0 0 0 0 0 0 654321 90 0 0 0 0 0 0";
        assert_eq!(parse_net_dev_line(l), Some(("eth0".to_string(), 123456, 654321)));
        assert_eq!(parse_net_dev_line("Inter-|   Receive"), None);
    }
}
