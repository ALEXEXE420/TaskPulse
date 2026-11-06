//! Kleine Hilfen zum Lesen von Linux-Systemdateien (/proc, /sys). Unter Windows ungenutzt.
#![allow(dead_code)]

use std::path::Path;

/// Datei lesen und Leerraum abschneiden.
pub fn read(p: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

pub fn read_u64(p: impl AsRef<Path>) -> Option<u64> {
    read(p)?.parse().ok()
}

pub fn read_i64(p: impl AsRef<Path>) -> Option<i64> {
    read(p)?.parse().ok()
}

pub fn read_f64(p: impl AsRef<Path>) -> Option<f64> {
    read(p)?.parse().ok()
}

/// Listen wie "0-3,8,10-11" (z. B. shared_cpu_list) → [0,1,2,3,8,10,11]
pub fn parse_cpu_list(s: &str) -> Vec<usize> {
    let mut v = Vec::new();
    for part in s.trim().split(',').filter(|p| !p.is_empty()) {
        match part.split_once('-') {
            Some((a, b)) => {
                if let (Ok(a), Ok(b)) = (a.trim().parse::<usize>(), b.trim().parse::<usize>()) {
                    v.extend(a..=b);
                }
            }
            None => {
                if let Ok(a) = part.trim().parse() {
                    v.push(a);
                }
            }
        }
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// Größenangaben wie "32768K", "32M" → Bytes
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mul) = match s.chars().last()? {
        'K' | 'k' => (&s[..s.len() - 1], 1024),
        'M' | 'm' => (&s[..s.len() - 1], 1024 * 1024),
        'G' | 'g' => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        _ => (s, 1),
    };
    num.trim().parse::<u64>().ok().map(|n| n * mul)
}

/// Alle Unterordner (z. B. /sys/class/hwmon/hwmon*) als Pfade
pub fn dirs(p: impl AsRef<Path>) -> Vec<std::path::PathBuf> {
    let mut v: Vec<_> = std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_lists_and_sizes() {
        assert_eq!(parse_cpu_list("0-3,8,10-11\n"), vec![0, 1, 2, 3, 8, 10, 11]);
        assert_eq!(parse_cpu_list("5"), vec![5]);
        assert_eq!(parse_cpu_list(""), Vec::<usize>::new());
        assert_eq!(parse_size("32768K"), Some(32 * 1024 * 1024));
        assert_eq!(parse_size("1024"), Some(1024));
    }
}
