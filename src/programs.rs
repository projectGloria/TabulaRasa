//! Reads the list of installed programs from the Windows registry.
//! Rules come from MECHANICS.md (scan routine of the original at 0x850e4c).

use std::path::{Path, PathBuf};
use winreg::RegKey;
use winreg::enums::*;

pub const UNINSTALL_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
pub const UNINSTALL_KEY_WOW: &str = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";

/// KEY_READ | KEY_WOW64_64KEY = 0x20119, as the original opens its keys.
pub const READ_FLAGS: u32 = KEY_READ | KEY_WOW64_64KEY;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Machine64,
    Machine32,
    CurrentUser,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Machine64 => "All users (64-bit)",
            Source::Machine32 => "All users (32-bit)",
            Source::CurrentUser => "Current user",
        }
    }
    pub fn root_name(self) -> &'static str {
        match self {
            Source::CurrentUser => "HKEY_CURRENT_USER",
            _ => "HKEY_LOCAL_MACHINE",
        }
    }
    pub fn hive(self) -> RegKey {
        match self {
            Source::CurrentUser => RegKey::predef(HKEY_CURRENT_USER),
            _ => RegKey::predef(HKEY_LOCAL_MACHINE),
        }
    }
    pub fn base_key(self) -> &'static str {
        match self {
            Source::Machine32 => UNINSTALL_KEY_WOW,
            _ => UNINSTALL_KEY,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Program {
    pub id: usize,
    pub name: String,
    pub publisher: String,
    pub version: String,
    /// yyyy-mm-dd, empty when unknown
    pub install_date: String,
    pub date_from_key_time: bool,
    pub size: Option<u64>,
    pub size_from_folder: bool,
    pub install_location: String,
    pub uninstall_cmd: String,
    pub icon: Option<(String, i32)>,
    pub source: Source,
    pub key_name: String,
    pub system_component: bool,
    pub is_msi: bool,
    pub help_link: String,
    pub about_url: String,
    pub update_url: String,
    pub comments: String,
}

impl Program {
    pub fn key_subpath(&self) -> String {
        format!(r"{}\{}", self.source.base_key(), self.key_name)
    }
    pub fn key_full_path(&self) -> String {
        format!(r"{}\{}", self.source.root_name(), self.key_subpath())
    }
    pub fn key_still_exists(&self) -> bool {
        self.source
            .hive()
            .open_subkey_with_flags(self.key_subpath(), READ_FLAGS)
            .is_ok()
    }
    pub fn website(&self) -> Option<&str> {
        [&self.about_url, &self.help_link, &self.update_url]
            .into_iter()
            .map(|s| s.trim())
            .find(|s| s.starts_with("http"))
    }
}

fn get_str(key: &RegKey, name: &str) -> String {
    key.get_value::<String, _>(name)
        .map(|s| s.trim().trim_end_matches('\0').to_string())
        .unwrap_or_default()
}

fn get_dword(key: &RegKey, name: &str) -> Option<u32> {
    key.get_value::<u32, _>(name).ok()
}

/// Splits a command line into program and arguments.
pub fn split_command(cmd: &str) -> (String, String) {
    let cmd = cmd.trim();
    if let Some(rest) = cmd.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return (rest[..end].to_string(), rest[end + 1..].trim().to_string());
        }
        return (rest.to_string(), String::new());
    }
    let lower = cmd.to_ascii_lowercase();
    if let Some(pos) = lower.find(".exe") {
        let end = pos + 4;
        return (cmd[..end].to_string(), cmd[end..].trim().to_string());
    }
    match cmd.find(' ') {
        Some(p) => (cmd[..p].to_string(), cmd[p + 1..].trim().to_string()),
        None => (cmd.to_string(), String::new()),
    }
}

/// "C:\x\app.exe,0" or "\"C:\x.exe\",-101" -> (path, index)
pub fn parse_icon(raw: &str) -> Option<(String, i32)> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (path, idx) = match raw.rfind(',') {
        Some(p) if raw[p + 1..].trim().parse::<i32>().is_ok() => {
            (&raw[..p], raw[p + 1..].trim().parse::<i32>().unwrap())
        }
        _ => (raw, 0),
    };
    let path = path.trim().trim_matches('"').to_string();
    let path = expand_env(&path);
    if Path::new(&path).is_file() {
        Some((path, idx))
    } else {
        None
    }
}

pub fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find('%') {
            let var = &after[..end];
            match std::env::var(var) {
                Ok(v) if !var.is_empty() => out.push_str(&v),
                _ => {
                    out.push('%');
                    out.push_str(var);
                    out.push('%');
                }
            }
            rest = &after[end + 1..];
        } else {
            out.push_str(&rest[start..]);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

fn is_msiexec(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with("msiexec.exe") || path.eq_ignore_ascii_case("msiexec")
}

fn dir_of_existing_file(path: &str) -> Option<PathBuf> {
    let p = Path::new(path);
    if p.is_file() {
        p.parent().map(|d| d.to_path_buf())
    } else {
        None
    }
}

/// Converts a FILETIME (100 ns since 1601) into yyyy-mm-dd.
fn filetime_to_date(low: u32, high: u32) -> String {
    let ticks = ((high as u64) << 32) | low as u64;
    if ticks == 0 {
        return String::new();
    }
    let secs = ticks / 10_000_000;
    let days = (secs / 86_400) as i64 - 134_774; // days 1601-01-01 .. 1970-01-01
    civil_from_days(days)
}

fn civil_from_days(z: i64) -> String {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn normalize_date(raw: &str) -> String {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() == 8 && !raw.contains('/') {
        let (y, m, d) = (&digits[0..4], &digits[4..6], &digits[6..8]);
        if y.starts_with("19") || y.starts_with("20") {
            return format!("{y}-{m}-{d}");
        }
    }
    // m/d/yyyy as some installers write it
    let parts: Vec<&str> = raw.split('/').collect();
    if parts.len() == 3 {
        if let (Ok(m), Ok(d), Ok(y)) = (
            parts[0].trim().parse::<u32>(),
            parts[1].trim().parse::<u32>(),
            parts[2].trim().parse::<u32>(),
        ) {
            return format!("{y:04}-{m:02}-{d:02}");
        }
    }
    String::new()
}

fn read_entry(source: Source, base: &RegKey, key_name: &str) -> Option<Program> {
    let key = base.open_subkey_with_flags(key_name, READ_FLAGS).ok()?;
    let uninstall_cmd = get_str(&key, "UninstallString");
    // An entry needs an uninstall command to be listed at all.
    if uninstall_cmd.is_empty() {
        return None;
    }
    let display_name = get_str(&key, "DisplayName");
    let system_component = get_dword(&key, "SystemComponent") == Some(1)
        || display_name.is_empty()
        || !get_str(&key, "ParentKeyName").is_empty();
    let name = if display_name.is_empty() { key_name.to_string() } else { display_name };

    let is_msi = get_dword(&key, "WindowsInstaller") == Some(1);
    let mut uninstall_cmd = uninstall_cmd;
    if is_msi && key_name.starts_with('{') && key_name.ends_with('}') {
        uninstall_cmd = format!("MsiExec.exe /X{key_name}");
    }

    // Install location: InstallLocation, else the folder of the uninstaller, else of the icon.
    let mut install_location = expand_env(get_str(&key, "InstallLocation").trim_matches('"'));
    install_location = install_location.trim_end_matches('\\').to_string();
    if install_location.is_empty() || !Path::new(&install_location).is_dir() {
        let (exe, _) = split_command(&get_str(&key, "UninstallString"));
        let exe = expand_env(&exe);
        let mut derived = None;
        if !is_msiexec(&exe) {
            derived = dir_of_existing_file(&exe);
        }
        if derived.is_none() {
            if let Some((icon, _)) = parse_icon(&get_str(&key, "DisplayIcon")) {
                if !is_msiexec(&icon) {
                    derived = dir_of_existing_file(&icon);
                }
            }
        }
        if let Some(d) = derived {
            if install_location.is_empty() {
                install_location = d.to_string_lossy().to_string();
            }
        }
    }

    let mut install_date = normalize_date(&get_str(&key, "InstallDate"));
    let mut date_from_key_time = false;
    if install_date.is_empty() {
        if let Ok(info) = key.query_info() {
            install_date = filetime_to_date(
                info.last_write_time.dwLowDateTime,
                info.last_write_time.dwHighDateTime,
            );
            date_from_key_time = !install_date.is_empty();
        }
    }

    // EstimatedSize is in kilobytes.
    let size = get_dword(&key, "EstimatedSize")
        .filter(|&kb| kb > 0)
        .map(|kb| kb as u64 * 1024);

    let icon = parse_icon(&get_str(&key, "DisplayIcon")).or_else(|| {
        let (exe, _) = split_command(&get_str(&key, "UninstallString"));
        let exe = expand_env(&exe);
        if !is_msiexec(&exe) && Path::new(&exe).is_file() { Some((exe, 0)) } else { None }
    });

    Some(Program {
        id: 0,
        name,
        publisher: get_str(&key, "Publisher"),
        version: get_str(&key, "DisplayVersion"),
        install_date,
        date_from_key_time,
        size,
        size_from_folder: false,
        install_location,
        uninstall_cmd,
        icon,
        source,
        key_name: key_name.to_string(),
        system_component,
        is_msi,
        help_link: get_str(&key, "HelpLink"),
        about_url: get_str(&key, "URLInfoAbout"),
        update_url: get_str(&key, "URLUpdateInfo"),
        comments: get_str(&key, "Comments"),
    })
}

/// Scans the three uninstall locations in the original's order.
pub fn scan() -> Vec<Program> {
    let mut out: Vec<Program> = Vec::new();
    for source in [Source::Machine64, Source::Machine32, Source::CurrentUser] {
        let Ok(base) = source.hive().open_subkey_with_flags(source.base_key(), READ_FLAGS) else {
            continue;
        };
        let names: Vec<String> = base.enum_keys().filter_map(Result::ok).collect();
        for key_name in names {
            if let Some(p) = read_entry(source, &base, &key_name) {
                // Same program registered twice (same name and version): keep the first.
                let dup = out.iter().any(|o| {
                    o.name.eq_ignore_ascii_case(&p.name)
                        && o.version == p.version
                        && o.system_component == p.system_component
                });
                if !dup {
                    out.push(p);
                }
            }
        }
    }
    out.sort_by_key(|p| p.name.to_lowercase());
    for (i, p) in out.iter_mut().enumerate() {
        p.id = i;
    }
    out
}

/// Size of a folder, not following links. Stops after `max_entries` entries.
pub fn folder_size(path: &Path, max_entries: usize) -> Option<u64> {
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    let mut seen = 0usize;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            seen += 1;
            if seen > max_entries {
                return Some(total);
            }
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push(entry.path());
            } else if let Ok(md) = entry.metadata() {
                total += md.len();
            }
        }
    }
    Some(total)
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if v >= 100.0 { format!("{v:.0} {}", UNITS[u]) } else { format!("{v:.1} {}", UNITS[u]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_command() {
        assert_eq!(
            split_command(r#""C:\Program Files\App\uninstall.exe" /all /silent"#),
            (r"C:\Program Files\App\uninstall.exe".to_string(), "/all /silent".to_string())
        );
        assert_eq!(
            split_command(r#""C:\Program Files\App\uninstall.exe""#),
            (r"C:\Program Files\App\uninstall.exe".to_string(), String::new())
        );
        assert_eq!(
            split_command(r"C:\App\uninst.exe -s"),
            (r"C:\App\uninst.exe".to_string(), "-s".to_string())
        );
        assert_eq!(
            split_command("uninstall.exe"),
            ("uninstall.exe".to_string(), String::new())
        );
        assert_eq!(
            split_command("cmd.exe /c del"),
            ("cmd.exe".to_string(), "/c del".to_string())
        );
    }

    #[test]
    fn test_normalize_date() {
        assert_eq!(normalize_date("20240512"), "2024-05-12");
        assert_eq!(normalize_date("5/12/2024"), "2024-05-12");
        assert_eq!(normalize_date("1/2/2023"), "2023-01-02");
        assert_eq!(normalize_date("invalid"), "");
        assert_eq!(normalize_date(""), "");
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500 bytes");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn test_civil_from_days() {
        assert_eq!(civil_from_days(0), "1970-01-01");
        assert_eq!(civil_from_days(19856), "2024-05-13");
    }
}
