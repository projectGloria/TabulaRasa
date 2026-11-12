//! Finds what a program leaves behind after its own uninstaller ran, and removes it.
//! Only exact folder / key names are matched, never partial names, so other programs'
//! files are not touched. Files go to the Recycle Bin; registry keys are saved to a
//! .reg backup before they are deleted.

use std::path::{Path, PathBuf};
use winreg::RegKey;
use winreg::enums::*;

use crate::programs::{Program, READ_FLAGS, folder_size};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Folder,
    File,
    RegistryKey,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Folder => "Folder",
            Kind::File => "Shortcut",
            Kind::RegistryKey => "Registry",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Leftover {
    pub kind: Kind,
    /// Folder/file path, or registry key below its root (for registry items).
    pub path: String,
    pub machine: bool,
    pub size: Option<u64>,
    pub selected: bool,
    pub reason: &'static str,
}

impl Leftover {
    pub fn display_path(&self) -> String {
        match self.kind {
            Kind::RegistryKey => format!(
                r"{}\{}",
                if self.machine { "HKEY_LOCAL_MACHINE" } else { "HKEY_CURRENT_USER" },
                self.path
            ),
            _ => self.path.clone(),
        }
    }
}

/// Words never used as a leftover name on their own.
const TOO_GENERIC: &[&str] = &[
    "microsoft", "windows", "common files", "programs", "program files", "programdata", "temp",
    "system32", "users", "appdata", "local", "roaming", "packages", "google", "mozilla", "intel",
    "nvidia", "amd", "apple", "adobe", "app", "bin", "data", "tools", "update", "updater",
    "classes", "policies", "wow6432node", "software", "uninstall", "setup", "installer",
];

fn is_generic(name: &str) -> bool {
    let n = name.trim().to_lowercase();
    n.chars().filter(|c| c.is_alphanumeric()).count() < 3 || TOO_GENERIC.contains(&n.as_str())
}

/// "7-Zip 23.01 (x64)" -> "7-Zip"; "Mozilla Firefox (x64 en-US)" -> "Mozilla Firefox"
pub fn name_stem(name: &str) -> String {
    let mut s = name.to_string();
    while let (Some(a), Some(b)) = (s.find('('), s.rfind(')')) {
        if a < b {
            s.replace_range(a..=b, " ");
        } else {
            break;
        }
    }
    let words: Vec<&str> = s.split_whitespace().collect();
    let mut keep = Vec::new();
    for w in words {
        let wl = w.to_lowercase();
        let digits_dots = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == '.');
        let versionish = digits_dots(&wl) || (wl.starts_with('v') && digits_dots(&wl[1..]));
        let arch = matches!(
            wl.as_str(),
            "x64" | "x86" | "64-bit" | "32-bit" | "-" | "\u{2013}" | "amd64" | "arm64" | "version" | "edition"
        );
        if versionish || arch {
            if keep.is_empty() {
                continue;
            }
            break;
        }
        keep.push(w);
    }
    keep.join(" ").trim_matches(|c: char| c == '-' || c == ',' || c.is_whitespace()).to_string()
}

fn publisher_stem(publisher: &str) -> String {
    let cut = [
        ",", " Inc", " LLC", " Ltd", " Corporation", " Corp", " GmbH", " Co.", " Limited", " S.A.",
        " B.V.", " AB", " Software",
    ];
    let mut s = publisher.trim().to_string();
    for c in cut {
        if let Some(p) = s.find(c) {
            if p > 0 {
                s.truncate(p);
            }
        }
    }
    s.trim().trim_end_matches('.').to_string()
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_dir())
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().trim_end_matches('\\').eq_ignore_ascii_case(b.to_string_lossy().trim_end_matches('\\'))
}

/// Folders that must never be removed, whatever a registry entry says.
fn protected_folders() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = [
        "ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "ProgramData", "APPDATA", "LOCALAPPDATA",
        "USERPROFILE", "PUBLIC", "SystemRoot", "windir", "SystemDrive", "CommonProgramFiles",
        "CommonProgramFiles(x86)", "TEMP",
    ]
    .iter()
    .filter_map(|v| std::env::var_os(v).map(PathBuf::from))
    .collect();
    if let Some(l) = env_path("LOCALAPPDATA") {
        v.push(l.join("Programs"));
        v.push(l.join("Temp"));
    }
    if let Some(a) = env_path("APPDATA") {
        v.push(a.join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Some(p) = env_path("ProgramData") {
        v.push(p.join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Some(u) = env_path("USERPROFILE") {
        if let Some(parent) = u.parent() {
            v.push(parent.to_path_buf());
        }
    }
    if let Some(sd) = std::env::var_os("SystemDrive") {
        let sd_str = sd.to_string_lossy();
        let drive = PathBuf::from(format!("{}:\\", sd_str.trim_end_matches([':', '\\'])));
        for sys_dir in ["Recovery", "System Volume Information", "Boot", "PerfLogs", "$Recycle.Bin", "Config.Msi"] {
            v.push(drive.join(sys_dir));
        }
    }
    v
}

pub fn is_safe_folder(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }

    let normal_count = path
        .components()
        .filter(|c| matches!(c, std::path::Component::Normal(_)))
        .count();
    if normal_count < 1 {
        return false;
    }

    let protected = protected_folders();
    if protected.iter().any(|p| same_path(p, path)) {
        return false;
    }

    for var in ["SystemRoot", "windir"] {
        if let Some(sys_root) = env_path(var) {
            if path.starts_with(&sys_root) {
                return false;
            }
        }
    }

    if let Some(u) = env_path("USERPROFILE") {
        if let Some(users_dir) = u.parent() {
            if path.parent().is_some_and(|p| same_path(p, users_dir)) {
                return false;
            }
        }
    }

    true
}

/// Looks for what `program` left behind. `others` are the other installed programs:
/// a folder another program also uses is never offered (the original warns about this).
pub fn scan(program: &Program, others: &[Program]) -> Vec<Leftover> {
    let mut found: Vec<Leftover> = Vec::new();
    let other_locations: Vec<PathBuf> = others
        .iter()
        .filter(|o| o.id != program.id && !o.install_location.is_empty())
        .map(|o| PathBuf::from(&o.install_location))
        .collect();
    let used_by_other = |p: &Path| {
        other_locations.iter().any(|o| same_path(o, p) || o.starts_with(p))
    };

    let add_folder = |found: &mut Vec<Leftover>, p: PathBuf, reason: &'static str| {
        if !p.is_dir() || !is_safe_folder(&p) || used_by_other(&p) {
            return;
        }
        if found.iter().any(|f| f.kind == Kind::Folder && (same_path(Path::new(&f.path), &p) || p.starts_with(&f.path))) {
            return;
        }
        found.retain(|f| !(f.kind != Kind::RegistryKey && Path::new(&f.path).starts_with(&p)));
        found.push(Leftover {
            kind: Kind::Folder,
            size: folder_size(&p, 200_000),
            path: p.to_string_lossy().to_string(),
            machine: false,
            selected: true,
            reason,
        });
    };

    // 1. The install folder itself, if the uninstaller left it.
    if !program.install_location.is_empty() {
        add_folder(&mut found, PathBuf::from(&program.install_location), "Install folder");
    }

    // 2. Folders with the program's name in the usual places.
    let mut names: Vec<String> = Vec::new();
    for n in [name_stem(&program.name), program.name.clone()] {
        if !n.is_empty() && !is_generic(&n) && !names.iter().any(|x| x.eq_ignore_ascii_case(&n)) {
            names.push(n);
        }
    }
    if let Some(last) = Path::new(&program.install_location).file_name() {
        let n = last.to_string_lossy().to_string();
        if !is_generic(&n) && !names.iter().any(|x| x.eq_ignore_ascii_case(&n)) {
            names.push(n);
        }
    }
    let publisher = publisher_stem(&program.publisher);

    let mut roots: Vec<PathBuf> = Vec::new();
    for v in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "ProgramData", "APPDATA", "LOCALAPPDATA"] {
        if let Some(p) = env_path(v) {
            if !roots.iter().any(|r| same_path(r, &p)) {
                roots.push(p);
            }
        }
    }
    if let Some(l) = env_path("LOCALAPPDATA") {
        roots.push(l.join("Programs"));
    }
    for root in &roots {
        for n in &names {
            add_folder(&mut found, root.join(n), "Program data");
            if !publisher.is_empty() {
                add_folder(&mut found, root.join(&publisher).join(n), "Program data");
            }
        }
    }

    // 3. Start menu folders and shortcuts.
    let mut menus = Vec::new();
    if let Some(a) = env_path("APPDATA") {
        menus.push(a.join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Some(p) = env_path("ProgramData") {
        menus.push(p.join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    let mut desktops = Vec::new();
    if let Some(u) = env_path("USERPROFILE") {
        desktops.push(u.join("Desktop"));
    }
    if let Some(p) = env_path("PUBLIC") {
        desktops.push(p.join("Desktop"));
    }
    for m in &menus {
        for n in &names {
            add_folder(&mut found, m.join(n), "Start menu");
        }
    }
    for dir in menus.iter().chain(desktops.iter()) {
        for n in &names {
            let lnk = dir.join(format!("{n}.lnk"));
            if lnk.is_file() && !found.iter().any(|f| lnk.starts_with(&f.path)) {
                found.push(Leftover {
                    kind: Kind::File,
                    size: std::fs::metadata(&lnk).ok().map(|m| m.len()),
                    path: lnk.to_string_lossy().to_string(),
                    machine: false,
                    selected: true,
                    reason: "Shortcut",
                });
            }
        }
    }

    // 4. Registry: the uninstall entry if it is still there, and settings keys by name.
    let add_key = |found: &mut Vec<Leftover>, machine: bool, sub: String, reason: &'static str| {
        let root = RegKey::predef(if machine { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER });
        let sub_l = sub.to_lowercase();
        let covered = found.iter().any(|f| {
            let fl = f.path.to_lowercase();
            f.kind == Kind::RegistryKey && f.machine == machine && (fl == sub_l || sub_l.starts_with(&format!("{fl}\\")))
        });
        if root.open_subkey_with_flags(&sub, READ_FLAGS).is_ok() && !covered {
            found.push(Leftover { kind: Kind::RegistryKey, path: sub, machine, size: None, selected: true, reason });
        }
    };
    if program.key_still_exists() {
        add_key(
            &mut found,
            program.source != crate::programs::Source::CurrentUser,
            program.key_subpath(),
            "Uninstall entry",
        );
    }
    for n in &names {
        for machine in [false, true] {
            add_key(&mut found, machine, format!(r"SOFTWARE\{n}"), "Settings");
            if machine {
                add_key(&mut found, machine, format!(r"SOFTWARE\WOW6432Node\{n}"), "Settings");
            }
            if !publisher.is_empty() {
                add_key(&mut found, machine, format!(r"SOFTWARE\{publisher}\{n}"), "Settings");
                if machine {
                    add_key(&mut found, machine, format!(r"SOFTWARE\WOW6432Node\{publisher}\{n}"), "Settings");
                }
            }
        }
    }
    found
}

pub fn backups_folder() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Tabula Rasa").join("Backups")
}

fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs.to_string()
}

/// Removes the selected items. Returns one line per item: (path, error if any).
pub fn remove(items: &[Leftover], program_name: &str) -> Vec<(String, Option<String>)> {
    let mut results = Vec::new();
    let backup_dir = backups_folder();
    let _ = std::fs::create_dir_all(&backup_dir);
    let safe_name: String = program_name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' })
        .collect();
    for (i, item) in items.iter().filter(|i| i.selected).enumerate() {
        let shown = item.display_path();
        let res = match item.kind {
            Kind::Folder | Kind::File => {
                let p = Path::new(&item.path);
                if item.kind == Kind::Folder && !is_safe_folder(p) {
                    Err("Protected folder, skipped".to_string())
                } else {
                    trash::delete(p).map_err(|e| e.to_string())
                }
            }
            Kind::RegistryKey => {
                let file = backup_dir.join(format!("{} {} {}.reg", safe_name.trim(), timestamp(), i));
                let _ = std::process::Command::new("reg")
                    .args(["export", &shown, &file.to_string_lossy(), "/y"])
                    .creation_flags_hidden()
                    .status();
                let root = RegKey::predef(if item.machine { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER });
                root.delete_subkey_all(&item.path).map_err(|e| e.to_string())
            }
        };
        results.push((shown, res.err()));
    }
    results
}

trait HiddenWindow {
    fn creation_flags_hidden(&mut self) -> &mut Self;
}

impl HiddenWindow for std::process::Command {
    fn creation_flags_hidden(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(0x0800_0000) // CREATE_NO_WINDOW
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems() {
        assert_eq!(name_stem("7-Zip 23.01 (x64)"), "7-Zip");
        assert_eq!(name_stem("Mozilla Firefox (x64 en-US)"), "Mozilla Firefox");
        assert_eq!(name_stem("Notepad++ (64-bit x64)"), "Notepad++");
        assert_eq!(name_stem("VLC media player"), "VLC media player");
        assert_eq!(name_stem("7-Zip 26.04 (x64 edition)"), "7-Zip");
        assert_eq!(name_stem("Ada version 1.1.8"), "Ada");
        assert_eq!(name_stem("Antigravity 2.31.0"), "Antigravity");
    }

    #[test]
    fn test_publisher_stems() {
        assert_eq!(publisher_stem("Mozilla Corporation"), "Mozilla");
        assert_eq!(publisher_stem("Valve Corp."), "Valve");
        assert_eq!(publisher_stem("Example Software, LLC"), "Example");
        assert_eq!(publisher_stem("Google LLC"), "Google");
        assert_eq!(publisher_stem("Acme Inc"), "Acme");
    }

    #[test]
    fn test_is_safe_folder() {
        // Drive roots should never be safe
        assert!(!is_safe_folder(Path::new("C:\\")));
        assert!(!is_safe_folder(Path::new("D:\\")));
        assert!(!is_safe_folder(Path::new("E:\\")));

        // Relative paths should never be safe
        assert!(!is_safe_folder(Path::new("relative\\path")));
        assert!(!is_safe_folder(Path::new("app")));

        // Windows system directories must never be safe
        if let Some(sys_root) = std::env::var_os("SystemRoot") {
            let p = PathBuf::from(&sys_root);
            assert!(!is_safe_folder(&p));
            assert!(!is_safe_folder(&p.join("System32")));
            assert!(!is_safe_folder(&p.join("SysWOW64")));
        }

        // Program Files root itself must not be safe
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            assert!(!is_safe_folder(&PathBuf::from(&pf)));
        }

        // User profile root must not be safe
        if let Some(user) = std::env::var_os("USERPROFILE") {
            let p = PathBuf::from(&user);
            assert!(!is_safe_folder(&p));
            if let Some(parent) = p.parent() {
                assert!(!is_safe_folder(parent));
            }
        }

        // Valid subdirectories on secondary drives or under Program Files should be safe
        assert!(is_safe_folder(Path::new("D:\\MyGame")));
        assert!(is_safe_folder(Path::new("D:\\Games\\MyGame")));
        assert!(is_safe_folder(Path::new("C:\\Program Files\\NonExistentAppTestFolder")));
    }

    /// Read-only: lists what a scan would offer for every installed program.
    #[test]
    #[ignore]
    fn dry_run_scan() {
        let all = crate::programs::scan();
        for p in all.iter().filter(|p| !p.system_component) {
            for l in super::scan(p, &all) {
                println!("{} | {:?} | {} | {}", p.name, l.kind, l.display_path(), l.reason);
            }
        }
    }
}
