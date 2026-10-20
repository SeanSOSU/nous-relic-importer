//! First run copies the app into the user's data folder, adds it to the app menu and to the desktop's login
//! autostart, and gives the copy the one capability it needs, CAP_NET_RAW, through the desktop's own password
//! prompt (pkexec). Asked once: the capability stays on the file, so starting it later asks nothing. An
//! update replaces the file and asks again.

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::APP_NAME;

const ID: &str = "starrailsim-sync";
const ICON: &[u8] = include_bytes!("../../assets/icon.png");

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// An XDG base directory: the variable when it is an absolute path, else the default under home.
fn xdg(variable: &str, default: &str) -> PathBuf {
    std::env::var_os(variable).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(default))
}

pub fn install_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join(ID)
}

/// Logs and the saved account.
pub fn data_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join(ID)
}

pub fn installed_exe() -> PathBuf {
    install_dir().join(ID)
}

fn menu_entry() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("applications").join(format!("{ID}.desktop"))
}

fn icon_file() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("icons/hicolor/256x256/apps").join(format!("{ID}.png"))
}

fn autostart_entry() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("autostart").join(format!("{ID}.desktop"))
}

pub fn running_installed() -> bool {
    let canonical = |p: PathBuf| std::fs::canonicalize(&p).unwrap_or(p);
    std::env::current_exe().map(canonical).ok() == Some(canonical(installed_exe()))
}

/// Whether the installed copy is byte-for-byte this executable: running it again is then a request to show
/// the window, not an update.
pub fn installed_is_current() -> bool {
    match (std::env::current_exe().and_then(std::fs::read), std::fs::read(installed_exe())) {
        (Ok(this), Ok(installed)) => this == installed,
        _ => false,
    }
}

/// An Exec value: each argument quoted, with the characters the desktop entry spec reserves inside quotes
/// escaped, then the backslashes doubled again for the file's own string escaping, and `%` (a field code)
/// doubled.
fn exec_line(exe: &Path, args: &[String]) -> String {
    let quote = |s: &str| {
        let mut out = String::from("\"");
        for c in s.chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                out.push('\\');
            }
            out.push(c);
        }
        out.push('"');
        out.replace('\\', "\\\\").replace('%', "%%")
    };
    std::iter::once(exe.to_string_lossy().into_owned()).chain(args.iter().cloned()).map(|a| quote(&a)).collect::<Vec<_>>().join(" ")
}

fn desktop_entry(exe: &Path, args: &[String], autostart: bool) -> String {
    let mut entry = format!(
        "[Desktop Entry]\nType=Application\nName={APP_NAME}\nComment=Keeps StarRailSim in step with your game account\n\
         Exec={}\nIcon={ID}\nTerminal=false\nCategories=Game;Utility;\n",
        exec_line(exe, args)
    );
    if autostart {
        entry.push_str("X-GNOME-Autostart-enabled=true\n");
    } else {
        entry.push_str(&format!(
            "Actions=Uninstall;\n\n[Desktop Action Uninstall]\nName=Uninstall\nExec={}\n",
            exec_line(exe, &["--uninstall".to_owned()])
        ));
    }
    entry
}

fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)
}

/// Copies this executable into place, adds the menu entry and its icon, and registers it to start at login.
/// Returns the installed path. The copy has no capability yet: see `grant_capture`.
pub fn install(startup_args: &[String]) -> io::Result<PathBuf> {
    let target = installed_exe();
    std::fs::create_dir_all(install_dir())?;
    // Beside the target and renamed over it: a running copy keeps its old file until it exits.
    let temporary = target.with_extension("new");
    std::fs::copy(std::env::current_exe()?, &temporary)?;
    std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&temporary, &target)?;
    if let Err(e) = write(&icon_file(), ICON) {
        tracing::warn!(%e, "no menu icon");
    }
    write(&menu_entry(), desktop_entry(&target, &[], false).as_bytes())?;
    register_startup(startup_args)?;
    Ok(target)
}

pub fn startup_registered() -> bool {
    autostart_entry().exists()
}

pub fn register_startup(args: &[String]) -> io::Result<()> {
    write(&autostart_entry(), desktop_entry(&installed_exe(), args, true).as_bytes())
}

pub fn unregister_startup() -> io::Result<()> {
    match std::fs::remove_file(autostart_entry()) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

pub enum Grant {
    Granted,
    /// The password prompt was dismissed or the password refused.
    Refused,
    /// No pkexec (polkit) on this system.
    Unavailable,
}

/// Asks for the administrator password, through the desktop's polkit prompt, to give `exe` CAP_NET_RAW.
pub fn grant_capture(exe: &Path) -> Grant {
    match Command::new("pkexec").arg(exe).arg("--grant-capture").status() {
        Ok(status) if status.success() => Grant::Granted,
        Ok(_) => Grant::Refused,
        Err(_) => Grant::Unavailable,
    }
}

/// The root side of `grant_capture`: sets the file capability on this executable, as `setcap cap_net_raw=ep`
/// would. Returns the process exit code.
pub fn set_own_capability() -> i32 {
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("--grant-capture is run by pkexec, as root");
        return 2;
    }
    let Ok(exe) = std::env::current_exe() else { return 1 };
    // struct vfs_cap_data, revision 2, effective: permitted = CAP_NET_RAW (13), nothing inheritable.
    let mut data = Vec::with_capacity(20);
    data.extend((0x0200_0000u32 | 1).to_le_bytes());
    data.extend((1u32 << 13).to_le_bytes());
    data.extend([0u8; 12]);
    let path = std::ffi::CString::new(exe.as_os_str().as_encoded_bytes()).unwrap_or_default();
    let set = unsafe { libc::setxattr(path.as_ptr(), c"security.capability".as_ptr(), data.as_ptr().cast(), data.len(), 0) };
    if set == 0 { 0 } else { 1 }
}

/// The command a reader without pkexec runs instead.
pub fn manual_grant(exe: &Path) -> String {
    format!("sudo setcap cap_net_raw=ep '{}'", exe.display())
}

/// Removes the menu entry, its icon, the login autostart, the installed copy and the logs and saved account.
pub fn uninstall() -> io::Result<()> {
    unregister_startup()?;
    for file in [menu_entry(), icon_file()] {
        std::fs::remove_file(file).ok();
    }
    for dir in [install_dir(), data_dir()] {
        match std::fs::remove_dir_all(&dir) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{desktop_entry, exec_line};
    use std::path::Path;

    #[test]
    fn exec_lines_quote_and_escape_as_the_spec_asks() {
        let exe = Path::new("/home/a b/.local/share/starrailsim-sync/starrailsim-sync");
        assert_eq!(
            exec_line(exe, &["--site".into(), "http://localhost:5173/import?a=$1".into()]),
            r#""/home/a b/.local/share/starrailsim-sync/starrailsim-sync" "--site" "http://localhost:5173/import?a=\\$1""#
        );
        let entry = desktop_entry(exe, &["--background".into()], true);
        assert!(entry.contains("X-GNOME-Autostart-enabled=true") && !entry.contains("Desktop Action"));
        assert!(desktop_entry(exe, &[], false).contains("[Desktop Action Uninstall]"));
    }
}
