//! First run copies the app into the user's programs folder, adds a Start menu shortcut, and registers a
//! logon task that starts it with administrator rights. A task rather than the Run key because the Run key
//! cannot start an elevated program without a UAC prompt at every sign-in; the task asks once, when it is
//! created, and the shortcut starts the app through it too.

use std::io;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::APP_NAME;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn install_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Programs").join(APP_NAME)
}

pub fn installed_exe() -> PathBuf {
    install_dir().join("starrailsim-sync.exe")
}

pub fn running_installed() -> bool {
    let canonical = |p: PathBuf| std::fs::canonicalize(&p).unwrap_or(p);
    std::env::current_exe().map(canonical).ok() == Some(canonical(installed_exe()))
}

/// Whether the installed copy is byte-for-byte this executable: running it again is then a request to open
/// the site, not an update, and the running copy keeps its game session.
pub fn installed_is_current() -> bool {
    match (std::env::current_exe().and_then(std::fs::read), std::fs::read(installed_exe())) {
        (Ok(this), Ok(installed)) => this == installed,
        _ => false,
    }
}

fn shortcut() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join(r"Microsoft\Windows\Start Menu\Programs").join(format!("{APP_NAME}.lnk"))
}

/// Copies this executable into place, adds the Start menu shortcut and registers it to start at sign-in.
/// Returns the installed path.
pub fn install(task_args: &str) -> io::Result<PathBuf> {
    let target = installed_exe();
    std::fs::create_dir_all(install_dir())?;
    std::fs::copy(std::env::current_exe()?, &target)?;
    if let Err(e) = add_shortcut(&target) {
        tracing::warn!(%e, "no Start menu shortcut");
    }
    if !crate::win::add_uninstall_entry(&target, &install_dir()) {
        tracing::warn!("no Installed apps entry");
    }
    register_startup(task_args)?;
    Ok(target)
}

fn add_shortcut(target: &std::path::Path) -> io::Result<()> {
    // Single-quoted PowerShell strings escape a quote by doubling it.
    let quote = |p: &std::path::Path| p.to_string_lossy().replace('\'', "''");
    let script = format!(
        "$s = (New-Object -ComObject WScript.Shell).CreateShortcut('{}'); $s.TargetPath = '{}'; $s.WorkingDirectory = '{}'; \
         $s.Description = 'Keeps StarRailSim in step with your game account'; $s.Save()",
        quote(&shortcut()),
        quote(target),
        quote(&install_dir()),
    );
    let status = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    if status.success() { Ok(()) } else { Err(io::Error::other("powershell could not write the shortcut")) }
}

fn schtasks(args: &[&str]) -> io::Result<bool> {
    let status = Command::new("schtasks.exe")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    Ok(status.success())
}

pub fn startup_registered() -> bool {
    schtasks(&["/Query", "/TN", APP_NAME]).unwrap_or(false)
}

pub fn register_startup(task_args: &str) -> io::Result<()> {
    let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(domain), Ok(name)) => format!("{domain}\\{name}"),
        (_, Ok(name)) => name,
        _ => return Err(io::Error::other("no signed-in user")),
    };
    let exe = installed_exe();
    let xml = task_xml(&user, &exe.to_string_lossy(), task_args);
    // schtasks reads task XML as UTF-16.
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
    let file = std::env::temp_dir().join("starrailsim-sync-task.xml");
    std::fs::write(&file, bytes)?;
    let created = schtasks(&["/Create", "/TN", APP_NAME, "/XML", &file.to_string_lossy(), "/F"]);
    std::fs::remove_file(&file).ok();
    if created? { Ok(()) } else { Err(io::Error::other("schtasks refused the task")) }
}

/// Starts the installed copy through its sign-in task, which already has administrator rights, so a launch
/// from the Start menu needs no UAC prompt. False if there is no task to run.
pub fn start_through_task() -> bool {
    schtasks(&["/Run", "/TN", APP_NAME]).unwrap_or(false)
}

pub fn unregister_startup() -> io::Result<()> {
    if schtasks(&["/Delete", "/TN", APP_NAME, "/F"])? { Ok(()) } else { Err(io::Error::other("schtasks could not delete the task")) }
}

/// Removes the logon task and the shortcut and, once this process has exited, the installed folder and the logs.
pub fn uninstall() -> io::Result<()> {
    if startup_registered() {
        unregister_startup()?;
    }
    std::fs::remove_file(shortcut()).ok();
    crate::win::remove_uninstall_entry();
    let (program, logs) = (install_dir(), crate::data_dir());
    Command::new("cmd.exe")
        .raw_arg(format!(
            "/C ping -n 3 127.0.0.1 >NUL & rmdir /S /Q \"{}\" & rmdir /S /Q \"{}\"",
            program.display(),
            logs.display()
        ))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()?;
    Ok(())
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

// No time limit and no battery rules: the defaults stop a task after 72 hours, and on battery.
fn task_xml(user: &str, exe: &str, args: &str) -> String {
    let (user, exe, args) = (escape(user), escape(exe), escape(args));
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Keeps StarRailSim's relic inventory in step with your game account.</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <Arguments>{args}</Arguments>
    </Exec>
  </Actions>
</Task>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::task_xml;

    #[test]
    fn task_xml_runs_the_installed_exe_elevated_with_no_time_limit() {
        let exe = r"C:\Users\R&D\AppData\Local\Programs\StarRailSim Sync\starrailsim-sync.exe";
        let args = r#"--background --site "http://localhost:5173/import""#;
        let user = r"DESKTOP\R&D <Sean>";
        let xml = task_xml(user, exe, args);
        let doc = roxmltree::Document::parse(&xml).expect("well-formed task XML");
        let text = |tag: &str| doc.descendants().find(|n| n.has_tag_name(tag)).and_then(|n| n.text()).unwrap_or_default().to_owned();
        assert_eq!([text("Command"), text("Arguments"), text("UserId")], [exe, args, user]);
        assert_eq!(
            [text("RunLevel"), text("LogonType"), text("ExecutionTimeLimit"), text("StopIfGoingOnBatteries")],
            ["HighestAvailable", "InteractiveToken", "PT0S", "false"]
        );
    }
}
