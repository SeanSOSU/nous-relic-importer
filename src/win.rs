//! The few Win32 calls the app needs: elevation, one running copy, and a message box.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr::null;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, GetCurrentProcess, INFINITE, OpenEventW, OpenProcessToken, SetEvent,
    WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK, MessageBoxW, SW_SHOWNORMAL};

fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

pub fn is_elevated() -> bool {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut size = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            (&raw mut elevation).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

/// Starts this program again through the UAC prompt with the same arguments. False if the prompt was refused.
pub fn relaunch_elevated() -> bool {
    let exe = std::env::current_exe().unwrap_or_default();
    let args: Vec<String> = std::env::args().skip(1).map(|a| format!("\"{}\"", a.replace('"', "\\\""))).collect();
    let (verb, file, params) = (wide("runas"), wide(exe.as_os_str()), wide(args.join(" ")));
    let result = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), params.as_ptr(), null(), SW_SHOWNORMAL) };
    result as usize > 32
}

/// Held for the life of the running copy; dropping it lets the next one start.
pub struct SingleInstance(HANDLE);

impl SingleInstance {
    const NAME: &str = "Local\\NousRelicImporter";

    /// None when another copy is already running.
    pub fn acquire() -> Option<Self> {
        let name = wide(Self::NAME);
        unsafe {
            let handle = CreateMutexW(null(), 1, name.as_ptr());
            if handle.is_null() {
                return None;
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(handle);
                return None;
            }
            Some(Self(handle))
        }
    }

    pub fn other_running() -> bool {
        Self::acquire().is_none()
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// How another copy reaches the running one: QUIT lets a newer build step in (so updating is just running
/// it), SHOW brings the window up when the app is started again.
pub struct Signal(HANDLE);

impl Signal {
    pub const QUIT: &str = "Local\\NousRelicImporterQuit";
    pub const SHOW: &str = "Local\\NousRelicImporterShow";

    // The app runs elevated, and Windows keeps a normal-rights process from signalling an elevated one's
    // objects. This descriptor lets the signed-in user's own processes raise the signal (SYNCHRONIZE |
    // EVENT_MODIFY_STATE) and labels it medium integrity, so a plain double-click can reach it.
    const SDDL: &str = "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x100002;;;IU)S:(ML;;NW;;;ME)";

    pub fn create(name: &str) -> Option<Self> {
        let name = wide(name);
        let sddl = wide(Self::SDDL);
        unsafe {
            let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            let described =
                ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, std::ptr::null_mut()) != 0;
            let attributes = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            };
            let handle = CreateEventW(if described { &attributes } else { null() }, 0, 0, name.as_ptr());
            if described {
                LocalFree(descriptor);
            }
            (!handle.is_null()).then_some(Self(handle))
        }
    }

    /// Blocks until another copy raises this signal.
    pub fn wait(&self) {
        unsafe { WaitForSingleObject(self.0, INFINITE) };
    }

    /// Raises the running copy's signal. False when nothing is listening.
    pub fn send(name: &str) -> bool {
        let name = wide(name);
        unsafe {
            let handle = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
            if handle.is_null() {
                return false;
            }
            let raised = SetEvent(handle) != 0;
            CloseHandle(handle);
            raised
        }
    }

    /// Asks the running copy to quit and waits for it to let go. False if it is still running.
    pub fn quit_and_wait(timeout: std::time::Duration) -> bool {
        if !Self::send(Self::QUIT) {
            return false;
        }
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if !SingleInstance::other_running() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        false
    }
}

// An event handle may be waited on from any thread.
unsafe impl Send for Signal {}

impl Drop for Signal {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Nous Relic Importer";

/// Lists the app under Settings > Apps > Installed apps, uninstalling through `--uninstall`.
pub fn add_uninstall_entry(exe: &std::path::Path, install_dir: &std::path::Path) -> bool {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW,
        RegSetValueExW,
    };
    let exe = exe.to_string_lossy();
    let uninstall = format!("\"{exe}\" --uninstall");
    let text = [
        ("DisplayName", crate::APP_NAME.to_owned()),
        ("DisplayVersion", env!("CARGO_PKG_VERSION").to_owned()),
        ("Publisher", "StarRailSim".to_owned()),
        ("DisplayIcon", exe.to_string()),
        ("InstallLocation", install_dir.to_string_lossy().into_owned()),
        ("UninstallString", uninstall.clone()),
        ("QuietUninstallString", uninstall),
    ];
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        let path = wide(UNINSTALL_KEY);
        let created = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            null(),
            &mut key,
            std::ptr::null_mut(),
        );
        if created != 0 {
            return false;
        }
        let mut ok = true;
        for (name, value) in &text {
            let (name, value) = (wide(name), wide(value));
            ok &= RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, value.as_ptr().cast(), (value.len() * 2) as u32) == 0;
        }
        for name in ["NoModify", "NoRepair"] {
            let (name, one) = (wide(name), 1u32);
            ok &= RegSetValueExW(key, name.as_ptr(), 0, REG_DWORD, (&raw const one).cast(), 4) == 0;
        }
        RegCloseKey(key);
        ok
    }
}

/// Whether Installed apps lists the app: an update refreshes the entry only where the install made one.
pub fn has_uninstall_entry() -> bool {
    use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_READ, RegCloseKey, RegOpenKeyExW};
    let path = wide(UNINSTALL_KEY);
    let mut key: HKEY = std::ptr::null_mut();
    unsafe {
        if RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_READ, &mut key) != 0 {
            return false;
        }
        RegCloseKey(key);
    }
    true
}

pub fn remove_uninstall_entry() {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteTreeW};
    let path = wide(UNINSTALL_KEY);
    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, path.as_ptr()) };
}

pub fn lower_priority() {
    use windows_sys::Win32::System::Threading::{BELOW_NORMAL_PRIORITY_CLASS, SetPriorityClass};
    unsafe { SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS) };
}

pub fn message(text: &str) {
    let (text, title) = (wide(text), wide(crate::APP_NAME));
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONINFORMATION) };
}
