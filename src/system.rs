//! Small Windows helpers: starting programs (with the elevation prompt when needed),
//! waiting for them, reading icons and checking administrator rights.

use std::ffi::c_void;
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, QueryFullProcessImageNameW, TerminateProcess,
    INFINITE, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{
    ExtractIconExW, IsUserAnAdmin, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{DI_NORMAL, DestroyIcon, DrawIconEx, SW_SHOWNORMAL};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn is_admin() -> bool {
    unsafe { IsUserAnAdmin() != 0 }
}

pub struct Started(HANDLE);
unsafe impl Send for Started {}

impl Started {
    /// Waits until the program ends and returns its exit code.
    pub fn wait(self) -> u32 {
        unsafe {
            WaitForSingleObject(self.0, INFINITE);
            let mut code = 0u32;
            GetExitCodeProcess(self.0, &mut code);
            CloseHandle(self.0);
            code
        }
    }
}

/// Starts a program through the shell, so Windows can ask for administrator rights
/// when the program needs them.
pub fn shell_start(file: &str, params: &str, verb: Option<&str>) -> Result<Option<Started>, String> {
    let file_w = wide(file);
    let params_w = wide(params);
    let verb_w = verb.map(wide);
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb_w.as_ref().map_or(null(), |v| v.as_ptr());
        info.lpFile = file_w.as_ptr();
        info.lpParameters = if params.is_empty() { null() } else { params_w.as_ptr() };
        info.nShow = SW_SHOWNORMAL;
        if ShellExecuteExW(&mut info) == 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(1223) {
                return Err("The administrator prompt was cancelled.".into());
            }
            return Err(format!("Could not start \"{file}\": {err}"));
        }
        if info.hProcess.is_null() {
            Ok(None)
        } else {
            Ok(Some(Started(info.hProcess)))
        }
    }
}

pub fn open_path(path: &str) {
    let _ = shell_start(path, "", Some("open"));
}

pub fn open_url(url: &str) {
    let _ = shell_start(url, "", Some("open"));
}

/// Restarts Tabula Rasa with administrator rights. Returns true when the new copy started.
pub fn relaunch_as_admin() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    shell_start(&exe.to_string_lossy(), "", Some("runas")).is_ok()
}

/// Opens Registry Editor at the given key.
pub fn open_in_regedit(full_key: &str) {
    if let Ok(k) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).create_subkey(
        r"Software\Microsoft\Windows\CurrentVersion\Applets\Regedit",
    ) {
        let _ = k.0.set_value("LastKey", &format!(r"Computer\{full_key}"));
    }
    let _ = shell_start("regedit.exe", "-m", None);
}

/// Reads one icon from a program or icon file as 32x32 RGBA pixels.
pub fn icon_rgba(path: &str, index: i32) -> Option<(usize, Vec<u8>)> {
    const S: i32 = 32;
    let path_w = wide(path);
    unsafe {
        let mut large = null_mut();
        let n = ExtractIconExW(path_w.as_ptr(), index, &mut large, null_mut(), 1);
        if n == 0 || large.is_null() {
            return None;
        }
        let screen = GetDC(null_mut());
        let dc = CreateCompatibleDC(screen);
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = S;
        bmi.bmiHeader.biHeight = -S;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut c_void = null_mut();
        let dib = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        let mut result = None;
        if !dib.is_null() && !bits.is_null() {
            let old = SelectObject(dc, dib);
            let count = (S * S * 4) as usize;
            let buf = std::slice::from_raw_parts_mut(bits as *mut u8, count);
            // Draw on black and on white; the difference gives the transparency.
            buf.fill(0);
            DrawIconEx(dc, 0, 0, large, S, S, 0, null_mut(), DI_NORMAL);
            GdiFlush();
            let on_black = buf.to_vec();
            buf.fill(255);
            DrawIconEx(dc, 0, 0, large, S, S, 0, null_mut(), DI_NORMAL);
            GdiFlush();
            let on_white = buf.to_vec();
            SelectObject(dc, old);
            let mut rgba = vec![0u8; count];
            for i in 0..(S * S) as usize {
                let o = i * 4;
                let mut diff = 0i32;
                for c in 0..3 {
                    diff = diff.max(on_white[o + c] as i32 - on_black[o + c] as i32);
                }
                let a = (255 - diff).clamp(0, 255);
                rgba[o + 3] = a as u8;
                if a > 0 {
                    // BGR -> RGB, un-premultiply against black
                    for (dst, src) in [(0, 2), (1, 1), (2, 0)] {
                        rgba[o + dst] = ((on_black[o + src] as i32 * 255) / a).min(255) as u8;
                    }
                }
            }
            result = Some((S as usize, rgba));
            DeleteObject(dib);
        }
        DeleteDC(dc);
        ReleaseDC(null_mut(), screen);
        DestroyIcon(large);
        result
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningProcess {
    pub pid: u32,
    pub name: String,
    pub exe_path: Option<String>,
}

/// Enumerates running processes and returns any whose executable resides inside `dir`
/// or whose executable path matches `dir`.
pub fn find_processes_in_directory(dir: &Path) -> Vec<RunningProcess> {
    let mut found = Vec::new();
    let dir_str = dir.to_string_lossy().trim_end_matches('\\').to_lowercase();
    let dir_canon = std::fs::canonicalize(dir)
        .ok()
        .map(|p| p.to_string_lossy().trim_end_matches('\\').to_lowercase())
        .unwrap_or_default();

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return found;
        }

        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let pid = entry.th32ProcessID;
                if pid > 4 {
                    let name_len = entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len());
                    let name = String::from_utf16_lossy(&entry.szExeFile[..name_len]);

                    let h_proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                    let mut full_path: Option<String> = None;
                    if !h_proc.is_null() {
                        let mut buf = [0u16; 1024];
                        let mut size = buf.len() as u32;
                        if QueryFullProcessImageNameW(h_proc, 0, buf.as_mut_ptr(), &mut size) != 0 {
                            full_path = Some(String::from_utf16_lossy(&buf[..size as usize]));
                        }
                        CloseHandle(h_proc);
                    }

                    if let Some(ref path) = full_path {
                        let p_lower = path.to_lowercase();
                        let in_dir = p_lower.starts_with(&format!("{dir_str}\\"))
                            || p_lower == dir_str
                            || (!dir_canon.is_empty()
                                && (p_lower.starts_with(&format!("{dir_canon}\\")) || p_lower == dir_canon));
                        if in_dir {
                            found.push(RunningProcess {
                                pid,
                                name,
                                exe_path: full_path,
                            });
                        }
                    }
                }

                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
    }
    found
}

/// Terminates a process by its PID. Returns true if successful.
pub fn terminate_process_by_pid(pid: u32) -> bool {
    unsafe {
        let h_proc = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if h_proc.is_null() {
            return false;
        }
        let ok = TerminateProcess(h_proc, 1) != 0;
        CloseHandle(h_proc);
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_current_process_in_dir() {
        if let Ok(current_exe) = std::env::current_exe() {
            if let Some(parent) = current_exe.parent() {
                let procs = find_processes_in_directory(parent);
                let current_pid = std::process::id();
                assert!(
                    procs.iter().any(|p| p.pid == current_pid),
                    "Current running test executable should be discovered in its directory"
                );
            }
        }
    }
}
