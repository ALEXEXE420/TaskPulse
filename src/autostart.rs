//! Autostart über eine geplante Aufgabe mit höchsten Rechten (keine UAC-Abfrage beim Anmelden).
//! Gestartet wird nur die Taskleisten-Anzeige (`--taskbar`).

use crate::winutil;

pub const TASK_NAME: &str = "TaskPulse";

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Aufgabe für "immer mit Adminrechten": ohne Auslöser, wird von TaskPulse selbst gestartet.
pub const ADMIN_TASK_NAME: &str = "TaskPulse Admin";

pub fn task_xml(user: &str, exe: &str) -> String {
    build_xml(user, exe, "--taskbar", "TaskPulse – Taskleisten-Anzeige beim Anmelden starten", true, "IgnoreNew")
}

pub fn admin_task_xml(user: &str, exe: &str) -> String {
    build_xml(user, exe, "--elevated-launch", "TaskPulse – mit Adminrechten starten (ohne UAC-Abfrage)", false, "Parallel")
}

fn build_xml(user: &str, exe: &str, args: &str, desc: &str, logon: bool, instances: &str) -> String {
    let user = xml_escape(user);
    let exe = xml_escape(exe);
    let triggers = if logon {
        format!(
            r#"
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
      <Delay>PT3S</Delay>
    </LogonTrigger>
  </Triggers>"#
        )
    } else {
        String::new()
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>{desc}</Description>
  </RegistrationInfo>{triggers}
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>{instances}</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>"{exe}"</Command>
      <Arguments>{args}</Arguments>
    </Exec>
  </Actions>
</Task>
"#
    )
}

fn current_user() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    match std::env::var("USERDOMAIN") {
        Ok(d) if !d.is_empty() => format!("{d}\\{user}"),
        _ => user,
    }
}

/// Ist die Aufgabe eingerichtet?
pub fn is_enabled() -> bool {
    winutil::run_hidden("schtasks.exe", &["/Query", "/TN", TASK_NAME]) == Some(0)
}

fn run_schtasks(args: &str) -> Result<(), String> {
    let code = if winutil::is_elevated() {
        let parts = split_args(args);
        let refs: Vec<&str> = parts.iter().map(|s| s.as_str()).collect();
        winutil::run_hidden("schtasks.exe", &refs).map(|c| c as u32)
    } else {
        winutil::run_elevated_wait("schtasks.exe", args)
    };
    match code {
        Some(0) => Ok(()),
        Some(c) => Err(format!("schtasks beendet mit Code {c}")),
        None => Err("Abgebrochen (keine Adminrechte erteilt)".into()),
    }
}

fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut q = false;
    for c in s.chars() {
        match c {
            '"' => q = !q,
            ' ' if !q => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub fn enable() -> Result<(), String> {
    enable_for(&winutil::exe_path())
}

/// Autostart für eine bestimmte EXE (Installer: die installierte statt der laufenden).
pub fn enable_for(exe: &str) -> Result<(), String> {
    create_task(TASK_NAME, &task_xml(&current_user(), exe))
}

fn create_task(name: &str, xml: &str) -> Result<(), String> {
    let path = std::env::temp_dir().join(format!("{}.xml", name.replace(' ', "_")));
    // UTF-16 LE mit BOM, wie schtasks es erwartet
    let mut bytes = vec![0xFF, 0xFE];
    for u in xml.encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    let r = run_schtasks(&format!("/Create /TN \"{name}\" /XML \"{}\" /F", path.display()));
    let _ = std::fs::remove_file(&path);
    r
}

pub fn disable() -> Result<(), String> {
    if !is_enabled() {
        return Ok(());
    }
    run_schtasks(&format!("/Delete /TN {TASK_NAME} /F"))
}

/// Ist die Aufgabe "immer mit Adminrechten" eingerichtet (und zeigt auf diese EXE)?
pub fn admin_is_enabled() -> bool {
    winutil::run_hidden("schtasks.exe", &["/Query", "/TN", ADMIN_TASK_NAME]) == Some(0)
}

pub fn admin_enable() -> Result<(), String> {
    admin_enable_for(&winutil::exe_path())
}

pub fn admin_enable_for(exe: &str) -> Result<(), String> {
    create_task(ADMIN_TASK_NAME, &admin_task_xml(&current_user(), exe))
}

pub fn admin_disable() -> Result<(), String> {
    if !admin_is_enabled() {
        return Ok(());
    }
    run_schtasks(&format!("/Delete /TN \"{ADMIN_TASK_NAME}\" /F"))
}

/// Startet TaskPulse erhöht über die Aufgabe. Starten darf jeder Benutzer ohne Adminrechte.
pub fn run_admin_task() -> bool {
    winutil::run_hidden("schtasks.exe", &["/Run", "/TN", ADMIN_TASK_NAME]) == Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_contains_essentials() {
        let x = task_xml("PC\\alex", r"C:\Tools\TaskPulse & Co\TaskPulse.exe");
        assert!(x.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(x.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(x.contains("<Arguments>--taskbar</Arguments>"));
        assert!(x.contains("TaskPulse &amp; Co"));
        assert!(x.contains("<UserId>PC\\alex</UserId>"));
    }

    #[test]
    fn admin_xml_has_no_trigger() {
        let x = admin_task_xml("PC\\alex", r"C:\T\TaskPulse.exe");
        assert!(!x.contains("LogonTrigger"));
        assert!(x.contains("<Arguments>--elevated-launch</Arguments>"));
        assert!(x.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(x.contains("<MultipleInstancesPolicy>Parallel</MultipleInstancesPolicy>"));
    }

    #[test]
    fn args_split() {
        assert_eq!(split_args(r#"/Create /XML "C:\a b\x.xml" /F"#), vec!["/Create", "/XML", r"C:\a b\x.xml", "/F"]);
    }
}
