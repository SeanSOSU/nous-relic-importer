//! The few Win32 calls the app needs: elevation, one running copy, and a message box.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr::null;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, OpenProcessToken};
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
    const NAME: &str = "Local\\StarRailSimSync";

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

pub fn message(text: &str) {
    let (text, title) = (wide(text), wide(crate::APP_NAME));
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONINFORMATION) };
}
