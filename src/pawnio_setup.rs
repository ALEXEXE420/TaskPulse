//! PawnIO auf Knopfdruck installieren (nur Windows).
//!
//! Lädt den offiziellen Installer vom GitHub-Release des PawnIO-Entwicklers (namazso/PawnIO.Setup),
//! prüft die digitale Signatur (gültig und vom Herausgeber „namazso“) und startet ihn erst dann –
//! mit Adminrechten, sichtbar, sodass der Nutzer die Installation selbst bestätigt.

use std::sync::Mutex;

const URL: &str = "https://github.com/namazso/PawnIO.Setup/releases/latest/download/PawnIO_setup.exe";
/// Herausgeber laut Zertifikat (CN=namazso.eu, O=namazso)
const SIGNER: &str = "namazso";

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Idle,
    Working(&'static str),
    Done,
    Failed(String),
}

static STATE: Mutex<State> = Mutex::new(State::Idle);

/// Wurde PawnIO gerade installiert? Die Sensoren fragen das ab und versuchen es sofort erneut.
static JUST_INSTALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn state() -> State {
    STATE.lock().map(|s| s.clone()).unwrap_or(State::Idle)
}

fn set(s: State) {
    if let Ok(mut g) = STATE.lock() {
        *g = s;
    }
}

/// true (einmal), wenn PawnIO in diesem Lauf installiert wurde
pub fn take_just_installed() -> bool {
    JUST_INSTALLED.swap(false, std::sync::atomic::Ordering::Relaxed)
}

/// Installation im Hintergrund starten; `repaint` wird bei jeder Änderung aufgerufen.
pub fn start(repaint: impl Fn() + Send + 'static) {
    if matches!(state(), State::Working(_)) {
        return;
    }
    set(State::Working("PawnIO wird heruntergeladen …"));
    repaint();
    std::thread::spawn(move || {
        let r = install(&repaint);
        match &r {
            Ok(()) => {
                crate::diag::log("PawnIO: installiert");
                JUST_INSTALLED.store(true, std::sync::atomic::Ordering::Relaxed);
                set(State::Done);
            }
            Err(e) => {
                crate::diag::log(format!("PawnIO-Installation: {e}"));
                set(State::Failed(e.clone()));
            }
        }
        repaint();
    });
}

fn install(repaint: &dyn Fn()) -> Result<(), String> {
    let data = crate::updater::http_get(URL, "application/octet-stream", 32 * 1024 * 1024).map_err(|e| format!("Download fehlgeschlagen: {e}"))?;
    if data.len() < 100_000 || !data.starts_with(b"MZ") {
        return Err("Download unvollständig".into());
    }
    let dir = std::env::temp_dir().join("TaskPulse");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("PawnIO_setup.exe");
    std::fs::write(&path, &data).map_err(|e| format!("Speichern fehlgeschlagen: {e}"))?;
    set(State::Working("Signatur wird geprüft …"));
    repaint();
    match signer(&path) {
        Some(name) if name.contains(SIGNER) => {}
        Some(name) => {
            let _ = std::fs::remove_file(&path);
            return Err(format!("unerwarteter Herausgeber „{name}“ – Datei nicht gestartet"));
        }
        None => {
            let _ = std::fs::remove_file(&path);
            return Err("Signatur ungültig – Datei nicht gestartet".into());
        }
    }
    set(State::Working("PawnIO-Installer läuft – bitte die Installation bestätigen …"));
    repaint();
    let code = crate::winutil::run_elevated_wait_visible(&path.to_string_lossy(), "");
    let _ = std::fs::remove_file(&path);
    if crate::installer::pawnio_installed() {
        Ok(())
    } else {
        Err(match code {
            None => "abgebrochen (Adminrechte nicht erteilt)".into(),
            Some(c) => format!("Installation nicht abgeschlossen (Code {c})"),
        })
    }
}

/// Prüft die Authenticode-Signatur; bei gültiger Signatur der Name des Unterzeichners.
fn signer(path: &std::path::Path) -> Option<String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::{HANDLE, HWND};
    use windows::Win32::Security::Cryptography::{CertGetNameStringW, CERT_NAME_SIMPLE_DISPLAY_TYPE};
    use windows::Win32::Security::WinTrust::*;
    let p = HSTRING::from(path.as_os_str());
    unsafe {
        let mut file = WINTRUST_FILE_INFO {
            cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: PCWSTR(p.as_ptr()),
            hFile: HANDLE::default(),
            pgKnownSubject: std::ptr::null_mut(),
        };
        let mut data = WINTRUST_DATA {
            cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
            dwUIChoice: WTD_UI_NONE,
            fdwRevocationChecks: WTD_REVOKE_NONE,
            dwUnionChoice: WTD_CHOICE_FILE,
            Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
            dwStateAction: WTD_STATEACTION_VERIFY,
            ..Default::default()
        };
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        let ok = WinVerifyTrust(HWND::default(), &mut action, &mut data as *mut _ as *mut _) == 0;
        let mut name = None;
        if ok {
            let prov = WTHelperProvDataFromStateData(data.hWVTStateData);
            if !prov.is_null() {
                let sgnr = WTHelperGetProvSignerFromChain(prov, 0, false, 0);
                if !sgnr.is_null() && !(*sgnr).pasCertChain.is_null() {
                    let cert = (*(*sgnr).pasCertChain).pCert;
                    if !cert.is_null() {
                        let mut buf = [0u16; 256];
                        let n = CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, Some(&mut buf));
                        if n > 1 {
                            name = Some(String::from_utf16_lossy(&buf[..n as usize - 1]));
                        }
                    }
                }
            }
        }
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        WinVerifyTrust(HWND::default(), &mut action, &mut data as *mut _ as *mut _);
        name
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn signature_check() {
        // Unsignierte Datei (dieser Test selbst) → abgelehnt
        assert_eq!(super::signer(&std::env::current_exe().unwrap()), None);
        // Optional: echte PawnIO-Datei prüfen (Pfad über TASKPULSE_TEST_PAWNIO)
        if let Some(p) = std::env::var_os("TASKPULSE_TEST_PAWNIO") {
            let n = super::signer(std::path::Path::new(&p)).expect("Signatur");
            assert!(n.contains(super::SIGNER), "{n}");
        }
    }
}
